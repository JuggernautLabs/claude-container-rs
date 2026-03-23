//! Volume state types — each volume has a role and content contract.
//!
//! Five volumes per session, each with different invariants:
//!   session — git repos + config files (THE workspace)
//!   state   — Claude conversation history + settings
//!   cargo   — cargo cache (performance only)
//!   npm     — npm cache (performance only)
//!   pip     — pip cache (performance only)

use std::path::PathBuf;
use super::{VolumeName, CommitHash};

// ============================================================================
// Volume existence
// ============================================================================

/// The set of volumes for a session, with per-volume state
#[derive(Debug)]
pub struct SessionVolumes {
    pub session: VolumeState<SessionVolumeContent>,
    pub state: VolumeState<StateVolumeContent>,
    pub cargo: VolumeState<()>,
    pub npm: VolumeState<()>,
    pub pip: VolumeState<()>,
}

/// State of a single volume
#[derive(Debug)]
pub enum VolumeState<Content> {
    /// Volume exists with inspected content
    Present {
        name: VolumeName,
        content: Content,
    },
    /// Volume exists but content hasn't been inspected yet
    Exists {
        name: VolumeName,
    },
    /// Volume does not exist
    Missing {
        name: VolumeName,
    },
}

impl<C> VolumeState<C> {
    pub fn name(&self) -> &VolumeName {
        match self {
            Self::Present { name, .. } => name,
            Self::Exists { name } => name,
            Self::Missing { name } => name,
        }
    }

    pub fn exists(&self) -> bool {
        !matches!(self, Self::Missing { .. })
    }
}

impl SessionVolumes {
    pub fn all_exist(&self) -> bool {
        self.session.exists()
            && self.state.exists()
            && self.cargo.exists()
            && self.npm.exists()
            && self.pip.exists()
    }

    pub fn missing(&self) -> Vec<&VolumeName> {
        let mut m = vec![];
        if !self.session.exists() { m.push(self.session.name()); }
        if !self.state.exists() { m.push(self.state.name()); }
        if !self.cargo.exists() { m.push(self.cargo.name()); }
        if !self.npm.exists() { m.push(self.npm.name()); }
        if !self.pip.exists() { m.push(self.pip.name()); }
        m
    }
}

// ============================================================================
// Session volume content (the workspace)
// ============================================================================

/// What's inside the session volume (/workspace)
#[derive(Debug)]
pub struct SessionVolumeContent {
    /// Config file present?
    pub config: ConfigState,
    /// Main project marker
    pub main_project: Option<String>,
    /// Repos found in the volume
    pub repos: Vec<VolumeRepo>,
    /// Merge/sync markers
    pub markers: VolumeMarkers,
}

/// State of .claude-projects.yml
#[derive(Debug)]
pub enum ConfigState {
    Present {
        project_count: usize,
    },
    Missing,
    Invalid(String),
}

/// A repo found in the session volume
#[derive(Debug)]
pub struct VolumeRepo {
    pub name: String,
    pub head: CommitHash,
    pub dirty_files: u32,
    pub merging: bool,
    pub git_size_mb: u32,
}

/// Marker files that indicate in-progress operations
#[derive(Debug, Default)]
pub struct VolumeMarkers {
    /// .merge-into-branch — merge target in progress
    pub merge_into: Option<MergeIntoMarker>,
    /// .sync-branch — sync/rebase in progress
    pub sync: Option<SyncMarker>,
    /// .reconcile-complete — fin was called
    pub reconcile_complete: Option<String>,
    /// .agent-result — last agent run results
    pub agent_result: Option<String>,
    /// .repo-manifest — snapshot of repos at creation
    pub repo_manifest: bool,
}

#[derive(Debug)]
pub struct MergeIntoMarker {
    pub branch: String,
    pub has_summary: bool,
    pub has_mounts: bool,
}

#[derive(Debug)]
pub struct SyncMarker {
    pub branch: String,
    pub has_summary: bool,
}

// ============================================================================
// State volume content (Claude conversation history)
// ============================================================================

/// What's inside the state volume (/home/developer/.claude)
#[derive(Debug)]
pub struct StateVolumeContent {
    /// .claude.json (trust dialog, settings)
    pub claude_json: ClaudeJsonState,
    /// settings.json (statusline)
    pub settings_json: bool,
    /// Conversation history
    pub history: HistoryState,
    /// Projects with conversation data
    pub projects: Vec<String>,
}

#[derive(Debug)]
pub enum ClaudeJsonState {
    /// File exists with valid JSON
    Valid,
    /// File exists but is invalid JSON
    Invalid,
    /// File missing (first launch or corrupted)
    Missing,
}

#[derive(Debug)]
pub enum HistoryState {
    /// Has conversation history
    Present { entries: usize },
    /// No history file
    Empty,
}

// ============================================================================
// Volume health (for repair)
// ============================================================================

/// Problems found with a volume
#[derive(Debug)]
pub enum VolumeProblem {
    /// Volume missing entirely
    Missing(VolumeName),
    /// Config file missing from session volume
    NoConfig,
    /// No repos in session volume
    NoRepos,
    /// Repo has no .git directory
    BrokenRepo { name: String },
    /// Repo has uncommitted changes (may block operations)
    DirtyRepo { name: String, files: u32 },
    /// Merge in progress (blocks most operations)
    MergeInProgress { name: String },
    /// State volume has corrupted .claude.json
    CorruptedClaudeJson,
    /// Stale merge/sync markers (operation didn't complete)
    StaleMarker(String),
    /// Permission issues
    PermissionDenied { path: PathBuf },
}
