//! VM state — the world model.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// The full VM state: all repos + execution trace.
#[derive(Debug, Clone)]
pub struct SyncVM {
    pub session_name: String,
    pub target_branch: String,
    pub repos: BTreeMap<String, RepoVM>,
    pub trace: Vec<TraceEntry>,
}

/// Observed state of one repo across three reference points.
#[derive(Debug, Clone)]
pub struct RepoVM {
    /// Container HEAD
    pub container: RefState,
    /// Session branch HEAD on host
    pub session: RefState,
    /// Target branch HEAD on host (e.g., main)
    pub target: RefState,
    /// Container worktree state
    pub container_clean: bool,
    /// Host worktree state
    pub host_clean: bool,
    /// Host merge state
    pub host_merge_state: HostMergeState,
    /// Conflict state (for agent resolution)
    pub conflict: ConflictState,
    /// Host path for this repo
    pub host_path: Option<PathBuf>,
}

/// A git reference: either pointing at a commit or absent.
#[derive(Debug, Clone, PartialEq)]
pub enum RefState {
    /// Points at a known commit
    At(String),
    /// Branch/ref doesn't exist
    Absent,
}

impl RefState {
    pub fn hash(&self) -> Option<&str> {
        match self {
            Self::At(h) => Some(h),
            Self::Absent => None,
        }
    }

    pub fn is_present(&self) -> bool {
        matches!(self, Self::At(_))
    }
}

/// Host repo merge state.
#[derive(Debug, Clone, PartialEq)]
pub enum HostMergeState {
    Clean,
    Merging,
    Conflicted,
}

/// Container conflict state (for agent resolution).
#[derive(Debug, Clone, PartialEq)]
pub enum ConflictState {
    Clean,
    Markers(Vec<String>),
    Resolved,
}

/// Which side of the host/container divide.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Container,
    Host,
}

/// An entry in the execution trace.
#[derive(Debug, Clone)]
pub struct TraceEntry {
    pub op: super::Op,
    pub result: OpOutcome,
}

/// The outcome of executing an op (for trace).
#[derive(Debug, Clone)]
pub enum OpOutcome {
    Ok,
    OkWithValue(String),
    Conflict(Vec<String>),
    Failed(String),
    Skipped(String),
}

impl SyncVM {
    /// Create a new VM with no repos.
    pub fn new(session_name: &str, target_branch: &str) -> Self {
        Self {
            session_name: session_name.to_string(),
            target_branch: target_branch.to_string(),
            repos: BTreeMap::new(),
            trace: Vec::new(),
        }
    }

    /// Add or update a repo's state.
    pub fn set_repo(&mut self, name: &str, state: RepoVM) {
        self.repos.insert(name.to_string(), state);
    }

    /// Get a repo's state.
    pub fn repo(&self, name: &str) -> Option<&RepoVM> {
        self.repos.get(name)
    }

    /// Get a mutable reference to a repo's state.
    pub fn repo_mut(&mut self, name: &str) -> Option<&mut RepoVM> {
        self.repos.get_mut(name)
    }

    /// Record an operation in the trace.
    pub fn record(&mut self, op: super::Op, outcome: OpOutcome) {
        self.trace.push(TraceEntry { op, result: outcome });
    }
}

impl RepoVM {
    /// Create a new repo state with everything absent/clean.
    pub fn empty(host_path: Option<PathBuf>) -> Self {
        Self {
            container: RefState::Absent,
            session: RefState::Absent,
            target: RefState::Absent,
            container_clean: true,
            host_clean: true,
            host_merge_state: HostMergeState::Clean,
            conflict: ConflictState::Clean,
            host_path,
        }
    }

    /// Create from known state.
    pub fn from_refs(
        container: RefState,
        session: RefState,
        target: RefState,
        host_path: Option<PathBuf>,
    ) -> Self {
        Self {
            container,
            session,
            target,
            container_clean: true,
            host_clean: true,
            host_merge_state: HostMergeState::Clean,
            conflict: ConflictState::Clean,
            host_path,
        }
    }
}
