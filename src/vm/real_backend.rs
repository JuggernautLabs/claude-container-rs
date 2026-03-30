//! RealBackend — wraps SyncEngine methods to implement VmBackend.
//!
//! This is the bridge between the VM and the existing sync engine.
//! Each VmBackend method delegates to the corresponding SyncEngine
//! or git2 method.

use std::path::Path;
use crate::sync::SyncEngine;
use crate::types::SessionName;
use super::ops::{Mount, AgentTask, AncestryResult};
use super::backend::{VmBackend, VmBackendError};
use super::git2_ops::{open_repo, find_commit_hash, make_signature, write_ref, compare_trees, checkout_ref, create_commit};

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

    async fn merge_trees(&self, repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError> {
        // Use engine's trial_merge for in-memory merge
        let result = self.engine.trial_merge(
            repo_path,
            &crate::types::CommitHash::new(ours),
            &crate::types::CommitHash::new(theirs),
        );
        match result {
            Some(files) if files.is_empty() => {
                // Clean merge — compute the merged tree
                let repo = open_repo(repo_path)?;
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

    async fn checkout(&self, repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError> {
        let repo = open_repo(repo_path)?;
        checkout_ref(&repo, ref_name)
    }

    async fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], message: &str) -> Result<String, VmBackendError> {
        let repo = open_repo(repo_path)?;
        let sig = make_signature(&repo);
        create_commit(&repo, tree, parents, message, &sig)
    }

    async fn bundle_create(&self, _session: &str, _repo: &str) -> Result<String, VmBackendError> {
        // This would call engine.extract() internals — needs async + Docker
        // For now, return error. Full impl requires async VmBackend trait.
        Err(VmBackendError::Failed("bundle_create requires async runtime (not yet wired)".into()))
    }

    async fn bundle_fetch(&self, _repo_path: &Path, _bundle_path: &str) -> Result<String, VmBackendError> {
        Err(VmBackendError::Failed("bundle_fetch requires async runtime (not yet wired)".into()))
    }

    async fn run_container(&self, _image: &str, _script: &str, _mounts: &[Mount]) -> Result<(i64, String), VmBackendError> {
        Err(VmBackendError::Failed("run_container requires async runtime (not yet wired)".into()))
    }

    async fn extract(&self, _session: &str, repo: &str, host_path: &Path, session_branch: &str) -> Result<(u32, String), VmBackendError> {
        let result = self.engine.extract(&self.session, repo, host_path, session_branch).await
            .map_err(|e| VmBackendError::Failed(format!("extract: {}", e)))?;
        Ok((result.commit_count, result.new_head.to_string()))
    }

    async fn inject(&self, _session: &str, repo: &str, host_path: &Path, branch: &str) -> Result<(), VmBackendError> {
        self.engine.inject(&self.session, repo, host_path, branch).await
            .map_err(|e| VmBackendError::Failed(format!("inject: {}", e)))?;
        Ok(())
    }

    async fn force_inject(&self, _session: &str, _repo: &str, _host_path: &Path, _branch: &str) -> Result<(), VmBackendError> {
        // force_inject is private on SyncEngine — not yet wired.
        // When needed, SyncEngine.force_inject should be made pub.
        Err(VmBackendError::Failed("force_inject not yet wired (SyncEngine method is private)".into()))
    }

    async fn agent_run(&self, _task: &AgentTask, _context: &str, _mounts: &[Mount]) -> Result<(bool, Option<String>, Option<String>), VmBackendError> {
        Err(VmBackendError::Failed("agent_run requires async runtime (not yet wired)".into()))
    }

    async fn interactive_session(&self, _prompt: Option<&str>, _mounts: &[Mount]) -> Result<i64, VmBackendError> {
        Err(VmBackendError::Failed("interactive_session requires async runtime (not yet wired)".into()))
    }

    async fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError> {
        Ok(crate::confirm(message, false))
    }
}
