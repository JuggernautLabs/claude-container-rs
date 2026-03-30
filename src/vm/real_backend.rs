//! RealBackend — wraps SyncEngine methods to implement VmBackend.
//!
//! This is the bridge between the VM and the existing sync engine.
//! Each VmBackend method delegates to the corresponding SyncEngine
//! or git2 method.

use std::path::Path;
use git2::Repository;
use crate::sync::SyncEngine;
use crate::types::SessionName;
use super::ops::{Mount, AgentTask, AncestryResult};
use super::backend::{VmBackend, VmBackendError};

/// Backend that wraps SyncEngine for real git+docker operations.
pub struct RealBackend {
    engine: SyncEngine,
    session: SessionName,
}

impl RealBackend {
    pub fn new(engine: SyncEngine, session: SessionName) -> Self {
        Self { engine, session }
    }
}

impl VmBackend for RealBackend {
    fn ref_read(&self, repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError> {
        let repo = Repository::open(repo_path)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let reference = match repo.find_reference(ref_name) {
            Ok(r) => r,
            Err(_) => return Ok(None),
        };
        let commit = reference.peel_to_commit()
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(Some(commit.id().to_string()))
    }

    fn ref_write(&self, repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
        let repo = Repository::open(repo_path)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let oid = git2::Oid::from_str(hash)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        repo.reference(ref_name, oid, true, "vm: ref_write")
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(())
    }

    fn tree_compare(&self, repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
        let repo = Repository::open(repo_path)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let oid_a = git2::Oid::from_str(a).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let oid_b = git2::Oid::from_str(b).map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_a = repo.find_commit(oid_a).map_err(|e| VmBackendError::Failed(e.to_string()))?
            .tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_b = repo.find_commit(oid_b).map_err(|e| VmBackendError::Failed(e.to_string()))?
            .tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;

        if tree_a.id() == tree_b.id() {
            Ok((true, 0))
        } else {
            let diff = repo.diff_tree_to_tree(Some(&tree_a), Some(&tree_b), None)
                .map_err(|e| VmBackendError::Failed(e.to_string()))?;
            let stats = diff.stats().map_err(|e| VmBackendError::Failed(e.to_string()))?;
            Ok((false, stats.files_changed() as u32))
        }
    }

    fn ancestry_check(&self, repo_path: &Path, a: &str, b: &str) -> Result<AncestryResult, VmBackendError> {
        // Delegate to engine's check_ancestry which returns our Ancestry type
        let crate_ancestry = self.engine.check_ancestry(repo_path,
            &crate::types::CommitHash::new(a), &crate::types::CommitHash::new(b));
        // Convert from crate::types::Ancestry to vm::AncestryResult
        Ok(match crate_ancestry {
            crate::types::Ancestry::Same => AncestryResult::Same,
            crate::types::Ancestry::ContainerAhead { container_ahead } =>
                AncestryResult::BIsAncestorOfA { distance: container_ahead },
            crate::types::Ancestry::ContainerBehind { host_ahead } =>
                AncestryResult::AIsAncestorOfB { distance: host_ahead },
            crate::types::Ancestry::Diverged { container_ahead, host_ahead, merge_base } =>
                AncestryResult::Diverged {
                    a_ahead: host_ahead, b_ahead: container_ahead,
                    merge_base: merge_base.map(|h| h.to_string()),
                },
            crate::types::Ancestry::Unknown => AncestryResult::Unknown,
        })
    }

    fn merge_trees(&self, repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError> {
        // Use engine's trial_merge for in-memory merge
        let result = self.engine.trial_merge(
            repo_path,
            &crate::types::CommitHash::new(ours),
            &crate::types::CommitHash::new(theirs),
        );
        match result {
            Some(files) if files.is_empty() => {
                // Clean merge — compute the merged tree
                let repo = Repository::open(repo_path)
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;
                let oid_ours = git2::Oid::from_str(ours).map_err(|e| VmBackendError::Failed(e.to_string()))?;
                let oid_theirs = git2::Oid::from_str(theirs).map_err(|e| VmBackendError::Failed(e.to_string()))?;
                let ancestor_oid = repo.merge_base(oid_ours, oid_theirs)
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;

                let tree_ours = repo.find_commit(oid_ours).unwrap().tree().unwrap();
                let tree_theirs = repo.find_commit(oid_theirs).unwrap().tree().unwrap();
                let tree_ancestor = repo.find_commit(ancestor_oid).unwrap().tree().unwrap();

                let mut index = repo.merge_trees(&tree_ancestor, &tree_ours, &tree_theirs, None)
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;
                let tree_oid = index.write_tree_to(&repo)
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;
                Ok((true, Some(tree_oid.to_string()), vec![]))
            }
            Some(files) => Ok((false, None, files)),
            None => Err(VmBackendError::Failed("merge_trees: commits not available locally".into())),
        }
    }

    fn checkout(&self, repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError> {
        let repo = Repository::open(repo_path)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        repo.set_head(ref_name)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(())
    }

    fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], message: &str) -> Result<String, VmBackendError> {
        let repo = Repository::open(repo_path)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_oid = git2::Oid::from_str(tree)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_obj = repo.find_tree(tree_oid)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;

        let parent_commits: Vec<git2::Commit> = parents.iter()
            .map(|p| {
                let oid = git2::Oid::from_str(p).map_err(|e| VmBackendError::Failed(e.to_string()))?;
                repo.find_commit(oid).map_err(|e| VmBackendError::Failed(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let parent_refs: Vec<&git2::Commit> = parent_commits.iter().collect();

        let sig = repo.signature().unwrap_or_else(|_| {
            git2::Signature::now("git-sandbox", "git-sandbox@local").unwrap()
        });

        let oid = repo.commit(Some("HEAD"), &sig, &sig, message, &tree_obj, &parent_refs)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(oid.to_string())
    }

    fn bundle_create(&self, _session: &str, _repo: &str) -> Result<String, VmBackendError> {
        // This would call engine.extract() internals — needs async + Docker
        // For now, return error. Full impl requires async VmBackend trait.
        Err(VmBackendError::Failed("bundle_create requires async runtime (not yet wired)".into()))
    }

    fn bundle_fetch(&self, _repo_path: &Path, _bundle_path: &str) -> Result<String, VmBackendError> {
        Err(VmBackendError::Failed("bundle_fetch requires async runtime (not yet wired)".into()))
    }

    fn run_container(&self, _image: &str, _script: &str, _mounts: &[Mount]) -> Result<(i64, String), VmBackendError> {
        Err(VmBackendError::Failed("run_container requires async runtime (not yet wired)".into()))
    }

    fn agent_run(&self, _task: &AgentTask, _context: &str, _mounts: &[Mount]) -> Result<(bool, Option<String>, Option<String>), VmBackendError> {
        Err(VmBackendError::Failed("agent_run requires async runtime (not yet wired)".into()))
    }

    fn interactive_session(&self, _prompt: Option<&str>, _mounts: &[Mount]) -> Result<i64, VmBackendError> {
        Err(VmBackendError::Failed("interactive_session requires async runtime (not yet wired)".into()))
    }

    fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError> {
        Ok(crate::confirm(message, false))
    }
}
