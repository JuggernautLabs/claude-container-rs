//! SyncBackend implementation for SyncEngine — wraps existing methods.

use std::path::Path;
use git2::Repository;
use crate::backend::{SyncBackend, BackendError};
use crate::sync::SyncEngine;
use crate::types::git::MergeOutcome;

impl SyncBackend for SyncEngine {
    fn merge(
        &self,
        repo_path: &Path,
        from_branch: &str,
        to_branch: &str,
        squash: bool,
    ) -> Result<(), BackendError> {
        match self.merge(repo_path, from_branch, to_branch, squash) {
            Ok(MergeOutcome::Conflict { files }) => {
                Err(BackendError::Conflict { files })
            }
            Ok(_) => Ok(()),
            Err(e) => Err(BackendError::Failed(e.to_string())),
        }
    }

    fn branch_head(&self, repo_path: &Path, branch: &str) -> Option<String> {
        let repo = Repository::open(repo_path).ok()?;
        let refname = format!("refs/heads/{}", branch);
        let reference = repo.find_reference(&refname).ok()?;
        let commit = reference.peel_to_commit().ok()?;
        Some(commit.id().to_string())
    }

    fn is_worktree_clean(&self, repo_path: &Path) -> bool {
        let repo = match Repository::open(repo_path) {
            Ok(r) => r,
            Err(_) => return false,
        };
        let statuses = match repo.statuses(Some(
            git2::StatusOptions::new()
                .include_untracked(false)
                .include_ignored(false)
        )) {
            Ok(s) => s,
            Err(_) => return false,
        };
        statuses.is_empty()
    }

    fn has_conflict_markers(&self, repo_path: &Path, branch: &str) -> bool {
        let repo = match Repository::open(repo_path) {
            Ok(r) => r,
            Err(_) => return false,
        };
        let refname = format!("refs/heads/{}", branch);
        let target = match repo.find_reference(&refname)
            .and_then(|r| r.peel_to_commit()) {
            Ok(c) => c,
            Err(_) => return false,
        };
        let tree = match target.tree() {
            Ok(t) => t,
            Err(_) => return false,
        };
        let mut found = false;
        let _ = tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
            if found { return git2::TreeWalkResult::Abort; }
            if let Some(git2::ObjectType::Blob) = entry.kind() {
                if let Ok(blob) = repo.find_blob(entry.id()) {
                    let content = std::str::from_utf8(blob.content()).unwrap_or("");
                    if content.contains("<<<<<<<") || content.contains(">>>>>>>") {
                        found = true;
                        return git2::TreeWalkResult::Abort;
                    }
                }
            }
            git2::TreeWalkResult::Ok
        });
        found
    }

    fn commit_count(&self, repo_path: &Path, branch: &str) -> usize {
        let repo = match Repository::open(repo_path) {
            Ok(r) => r,
            Err(_) => return 0,
        };
        let refname = format!("refs/heads/{}", branch);
        let commit = match repo.find_reference(&refname)
            .and_then(|r| r.peel_to_commit()) {
            Ok(c) => c,
            Err(_) => return 0,
        };
        let mut revwalk = match repo.revwalk() {
            Ok(r) => r,
            Err(_) => return 0,
        };
        let _ = revwalk.push(commit.id());
        let _ = revwalk.set_sorting(git2::Sort::TOPOLOGICAL);
        let mut count = 0;
        for _ in revwalk { count += 1; }
        count
    }

    fn tree_id(&self, repo_path: &Path, branch: &str) -> Option<String> {
        let repo = Repository::open(repo_path).ok()?;
        let refname = format!("refs/heads/{}", branch);
        let reference = repo.find_reference(&refname).ok()?;
        let commit = reference.peel_to_commit().ok()?;
        let tree = commit.tree().ok()?;
        Some(tree.id().to_string())
    }
}
