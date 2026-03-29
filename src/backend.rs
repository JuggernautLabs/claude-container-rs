//! Backend trait — the interface between sync operations and git/docker.
//!
//! Tests call through this trait. Production uses `RealBackend` (wrapping
//! SyncEngine). The VM will implement this with primitives. Tests survive
//! any internal refactor because they assert on git state, not method returns.

use std::path::Path;

/// Errors from backend operations.
#[derive(Debug)]
pub enum BackendError {
    /// Merge had conflicts — not a failure, target was rolled back.
    Conflict { files: Vec<String> },
    /// Operation failed (git error, missing branch, etc.)
    Failed(String),
}

impl std::fmt::Display for BackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conflict { files } => write!(f, "conflict: {}", files.join(", ")),
            Self::Failed(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for BackendError {}

/// The interface that sync operations call through.
///
/// Merge is synchronous (git2 only). Extract and inject are async (Docker).
/// Observation methods are synchronous reads.
pub trait SyncBackend: Send + Sync {
    /// Merge `from_branch` into `to_branch` on the host repo.
    /// On conflict: target rolled back, returns Conflict.
    /// On success: target advanced.
    fn merge(
        &self,
        repo_path: &Path,
        from_branch: &str,
        to_branch: &str,
        squash: bool,
    ) -> Result<(), BackendError>;

    /// Read the HEAD commit hash of a branch. None if branch doesn't exist.
    fn branch_head(&self, repo_path: &Path, branch: &str) -> Option<String>;

    /// Check if the working tree has uncommitted changes.
    fn is_worktree_clean(&self, repo_path: &Path) -> bool;

    /// Check if any file on the given branch contains conflict markers.
    fn has_conflict_markers(&self, repo_path: &Path, branch: &str) -> bool;

    /// Count commits reachable from a branch.
    fn commit_count(&self, repo_path: &Path, branch: &str) -> usize;

    /// Get the tree OID for a branch (for content comparison).
    fn tree_id(&self, repo_path: &Path, branch: &str) -> Option<String>;
}
