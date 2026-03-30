//! Git2Backend — implements VmBackend using git2 for local git operations.
//!
//! No Docker. Handles ref ops, tree ops, merge, checkout, commit.
//! Transport ops (bundle) and container/agent ops return errors —
//! those require Docker and are tested separately.

use std::path::Path;
use git2::Repository;
use super::ops::{Mount, AncestryResult, AgentTask};
use super::backend::{VmBackend, VmBackendError};

/// A backend that operates on local git repos via git2.
/// No Docker dependency. Suitable for testing merge/ref/tree ops.
pub struct Git2Backend;

impl Git2Backend {
    pub fn new() -> Self { Self }
}

impl VmBackend for Git2Backend {
    async fn ref_read(&self, repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError> {
        let repo = open(repo_path)?;
        let reference = match repo.find_reference(ref_name) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        let commit = reference.peel_to_commit().map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(Some(commit.id().to_string()))
    }

    async fn ref_write(&self, repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
        let repo = open(repo_path)?;
        let oid = git2::Oid::from_str(hash).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        repo.reference(ref_name, oid, true, "vm: ref_write")
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(())
    }

    async fn tree_compare(&self, repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
        let repo = open(repo_path)?;
        let oid_a = git2::Oid::from_str(a).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let oid_b = git2::Oid::from_str(b).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let commit_a = repo.find_commit(oid_a).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let commit_b = repo.find_commit(oid_b).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_a = commit_a.tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_b = commit_b.tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;

        if tree_a.id() == tree_b.id() {
            Ok((true, 0))
        } else {
            let diff = repo.diff_tree_to_tree(Some(&tree_a), Some(&tree_b), None)
                .map_err(|e| VmBackendError::Failed(e.to_string()))?;
            let stats = diff.stats().map_err(|e| VmBackendError::Failed(e.to_string()))?;
            Ok((false, stats.files_changed() as u32))
        }
    }

    async fn ancestry_check(&self, repo_path: &Path, a: &str, b: &str) -> Result<AncestryResult, VmBackendError> {
        let repo = open(repo_path)?;
        let oid_a = git2::Oid::from_str(a).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let oid_b = git2::Oid::from_str(b).map_err(|e| VmBackendError::Failed(e.to_string()))?;

        if oid_a == oid_b {
            return Ok(AncestryResult::Same);
        }

        let a_is_ancestor = repo.graph_descendant_of(oid_b, oid_a).unwrap_or(false);
        let b_is_ancestor = repo.graph_descendant_of(oid_a, oid_b).unwrap_or(false);

        match (a_is_ancestor, b_is_ancestor) {
            (true, false) => {
                let count = count_between(&repo, oid_a, oid_b);
                Ok(AncestryResult::AIsAncestorOfB { distance: count })
            }
            (false, true) => {
                let count = count_between(&repo, oid_b, oid_a);
                Ok(AncestryResult::BIsAncestorOfA { distance: count })
            }
            (false, false) => {
                let merge_base = repo.merge_base(oid_a, oid_b).ok();
                let a_ahead = merge_base.map(|mb| count_between(&repo, mb, oid_a)).unwrap_or(0);
                let b_ahead = merge_base.map(|mb| count_between(&repo, mb, oid_b)).unwrap_or(0);
                Ok(AncestryResult::Diverged {
                    a_ahead, b_ahead,
                    merge_base: merge_base.map(|o| o.to_string()),
                })
            }
            (true, true) => Ok(AncestryResult::Same), // shouldn't happen if oids differ
        }
    }

    async fn merge_trees(&self, repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError> {
        let repo = open(repo_path)?;
        let oid_ours = git2::Oid::from_str(ours).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let oid_theirs = git2::Oid::from_str(theirs).map_err(|e| VmBackendError::Failed(e.to_string()))?;

        let commit_ours = repo.find_commit(oid_ours).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let commit_theirs = repo.find_commit(oid_theirs).map_err(|e| VmBackendError::Failed(e.to_string()))?;

        let ancestor_oid = repo.merge_base(oid_ours, oid_theirs)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let ancestor = repo.find_commit(ancestor_oid).map_err(|e| VmBackendError::Failed(e.to_string()))?;

        let tree_ours = commit_ours.tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_theirs = commit_theirs.tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_ancestor = ancestor.tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;

        let mut index = repo.merge_trees(&tree_ancestor, &tree_ours, &tree_theirs, None)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;

        if index.has_conflicts() {
            let conflicts: Vec<String> = index.conflicts()
                .map_err(|e| VmBackendError::Failed(e.to_string()))?
                .filter_map(|c| c.ok())
                .filter_map(|c| {
                    c.our.or(c.their).or(c.ancestor)
                        .and_then(|entry| String::from_utf8(entry.path).ok())
                })
                .collect();
            Ok((false, None, conflicts))
        } else {
            let tree_oid = index.write_tree_to(&repo)
                .map_err(|e| VmBackendError::Failed(e.to_string()))?;
            Ok((true, Some(tree_oid.to_string()), vec![]))
        }
    }

    async fn checkout(&self, repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError> {
        let repo = open(repo_path)?;
        repo.set_head(ref_name).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(())
    }

    async fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], message: &str) -> Result<String, VmBackendError> {
        let repo = open(repo_path)?;
        let tree_oid = git2::Oid::from_str(tree).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_obj = repo.find_tree(tree_oid).map_err(|e| VmBackendError::Failed(e.to_string()))?;

        let parent_commits: Vec<git2::Commit> = parents.iter()
            .map(|p| {
                let oid = git2::Oid::from_str(p).map_err(|e| VmBackendError::Failed(e.to_string()))?;
                repo.find_commit(oid).map_err(|e| VmBackendError::Failed(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let parent_refs: Vec<&git2::Commit> = parent_commits.iter().collect();

        let sig = repo.signature().unwrap_or_else(|_| {
            git2::Signature::now("vm", "vm@test.com").unwrap()
        });

        let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree_obj, &parent_refs)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;

        Ok(oid.to_string())
    }

    // Transport ops — require Docker, not available in Git2Backend
    async fn bundle_create(&self, _session: &str, _repo: &str) -> Result<String, VmBackendError> {
        Err(VmBackendError::Failed("bundle_create requires Docker (use MockBackend or RealBackend)".into()))
    }
    async fn bundle_fetch(&self, _repo_path: &Path, _bundle_path: &str) -> Result<String, VmBackendError> {
        Err(VmBackendError::Failed("bundle_fetch requires Docker (use MockBackend or RealBackend)".into()))
    }
    async fn run_container(&self, _image: &str, _script: &str, _mounts: &[Mount]) -> Result<(i64, String), VmBackendError> {
        Err(VmBackendError::Failed("run_container requires Docker".into()))
    }
    async fn agent_run(&self, _task: &AgentTask, _context: &str, _mounts: &[Mount]) -> Result<(bool, Option<String>, Option<String>), VmBackendError> {
        Err(VmBackendError::Failed("agent_run requires Docker".into()))
    }
    async fn interactive_session(&self, _prompt: Option<&str>, _mounts: &[Mount]) -> Result<i64, VmBackendError> {
        Err(VmBackendError::Failed("interactive_session requires Docker".into()))
    }
    async fn prompt_user(&self, _message: &str) -> Result<bool, VmBackendError> {
        Ok(true) // auto-confirm in tests
    }
}

fn open(path: &Path) -> Result<Repository, VmBackendError> {
    Repository::open(path).map_err(|e| VmBackendError::Failed(format!("open {}: {}", path.display(), e)))
}

fn count_between(repo: &Repository, from: git2::Oid, to: git2::Oid) -> u32 {
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let _ = revwalk.push(to);
    let _ = revwalk.hide(from);
    revwalk.count() as u32
}
