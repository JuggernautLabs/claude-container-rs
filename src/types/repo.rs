//! Repository sync types — per-repo state classification

use std::path::PathBuf;
use super::CommitHash;

/// A repo within a session
#[derive(Debug, Clone)]
pub struct RepoInfo {
    pub name: String,           // e.g. "hypermemetic/synapse"
    pub host_path: PathBuf,     // e.g. /Users/user/dev/hypermemetic/synapse
    pub container_head: Option<CommitHash>,
    pub session_head: Option<CommitHash>,
    pub target_head: Option<CommitHash>,
    pub squash_base: Option<CommitHash>,
    pub dirty_count: u32,       // uncommitted files in container
    pub merging: bool,          // MERGE_HEAD present in container
    pub host_dirty: bool,       // host repo has uncommitted changes
    pub container_known: bool,  // container HEAD exists in host repo
    pub container_in_target: bool, // container HEAD is ancestor of target
    pub external_ahead: u32,    // non-squash commits on target ahead of session
    pub extract_enabled: bool,  // false = discovered repo (extract: false in config)
    pub git_size_mb: u32,       // .git directory size
}

/// Sync classification for a repo — what action is needed
#[derive(Debug, Clone, PartialEq)]
pub enum SyncState {
    /// Both sides identical
    Identical,
    /// Content identical but histories differ (squash artifacts)
    SquashIdentical,
    /// Container has commits host doesn't
    ContainerAhead { commits: u32 },
    /// Host has commits container doesn't
    HostAhead { commits: u32 },
    /// Both sides have unique commits with content differences
    Diverged { container_ahead: u32, host_ahead: u32 },
    /// Repo only exists in container
    ContainerOnly,
    /// Repo only exists on host
    HostOnly,
    /// Container has uncommitted changes
    ContainerDirty { files: u32 },
    /// Host has uncommitted changes
    HostDirty,
    /// Repo is discovered but extract: false
    Discovered,
}

/// What action sync should take
#[derive(Debug, Clone, PartialEq)]
pub enum SyncAction {
    Skip,
    Pull,     // container → host (extract + merge)
    Push,     // host → container (ff)
    Reconcile, // merge host into container, then pull back
    CloneFromContainer,
    PushToContainer,
    Warn(String),
}

impl SyncState {
    /// Determine the action for this state
    pub fn action(&self) -> SyncAction {
        match self {
            Self::Identical | Self::SquashIdentical => SyncAction::Skip,
            Self::ContainerAhead { .. } => SyncAction::Pull,
            Self::HostAhead { .. } => SyncAction::Push,
            Self::Diverged { .. } => SyncAction::Reconcile,
            Self::ContainerOnly => SyncAction::CloneFromContainer,
            Self::HostOnly => SyncAction::PushToContainer,
            Self::ContainerDirty { files } =>
                SyncAction::Warn(format!("{} uncommitted file(s) in container", files)),
            Self::HostDirty =>
                SyncAction::Warn("host has uncommitted changes".into()),
            Self::Discovered =>
                SyncAction::Warn("extract: false (use pull --extract)".into()),
        }
    }
}
