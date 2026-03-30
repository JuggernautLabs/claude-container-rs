//! Git2Backend — implements VmBackend using git2 for local git operations.
//!
//! No Docker. Handles ref ops, tree ops, merge, checkout, commit.
//! Transport ops (bundle) and container/agent ops return errors —
//! those require Docker and are tested separately.

use std::path::Path;
use super::ops::{Mount, AncestryResult, AgentTask};
use super::backend::{VmBackend, VmBackendError};
use super::git2_ops::{open_repo, find_commit_hash, make_signature, write_ref, compare_trees, checkout_ref, create_commit, count_between};

/// A backend that operates on local git repos via git2.
/// No Docker dependency. Suitable for testing merge/ref/tree ops.
pub struct Git2Backend;

impl Git2Backend {
    pub fn new() -> Self { Self }
}

impl VmBackend for Git2Backend {
    async fn ref_read(&self, repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError> {
        let repo = open_repo(repo_path)?;
        find_commit_hash(&repo, ref_name)
    }

    async fn ref_write(&self, repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
        let repo = open_repo(repo_path)?;
        write_ref(&repo, ref_name, hash)
    }

    async fn tree_compare(&self, repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
        let repo = open_repo(repo_path)?;
        compare_trees(&repo, a, b)
    }

    async fn ancestry_check(&self, repo_path: &Path, a: &str, b: &str) -> Result<AncestryResult, VmBackendError> {
        let repo = open_repo(repo_path)?;
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
        let repo = open_repo(repo_path)?;
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
        let repo = open_repo(repo_path)?;
        checkout_ref(&repo, ref_name)
    }

    async fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], message: &str) -> Result<String, VmBackendError> {
        let repo = open_repo(repo_path)?;
        let sig = make_signature(&repo);
        create_commit(&repo, tree, parents, message, &sig)
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
    async fn extract(&self, _session: &str, _repo: &str, _host_path: &Path, _session_branch: &str) -> Result<(u32, String), VmBackendError> {
        Err(VmBackendError::Failed("extract requires Docker".into()))
    }
    async fn inject(&self, _session: &str, _repo: &str, _host_path: &Path, _branch: &str) -> Result<(), VmBackendError> {
        Err(VmBackendError::Failed("inject requires Docker".into()))
    }
    async fn force_inject(&self, _session: &str, _repo: &str, _host_path: &Path, _branch: &str) -> Result<(), VmBackendError> {
        Err(VmBackendError::Failed("force_inject requires Docker".into()))
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
