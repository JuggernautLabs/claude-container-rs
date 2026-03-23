//! Git state types — every possible relationship between branches.
//!
//! A repo exists in two places: container and host. Each side has its own
//! git state. The combination of both sides determines what sync action
//! is needed.

use std::path::PathBuf;
use super::CommitHash;

// ============================================================================
// Single-side git state (what one copy of a repo looks like)
// ============================================================================

/// The state of a git repo on one side (container or host)
#[derive(Debug, Clone)]
pub enum RepoPresence {
    /// Repo exists with commits
    Present(GitRepoState),
    /// Directory exists but no .git
    NotARepo(PathBuf),
    /// Path doesn't exist at all
    Missing,
}

/// State of a git repo that exists and has commits
#[derive(Debug, Clone)]
pub struct GitRepoState {
    /// Current HEAD commit
    pub head: CommitHash,
    /// Number of uncommitted changes (0 = clean)
    pub dirty_files: u32,
    /// Merge in progress (MERGE_HEAD exists)
    pub merging: bool,
    /// Rebase in progress (.git/rebase-merge or .git/rebase-apply exists)
    pub rebasing: bool,
}

impl GitRepoState {
    pub fn is_clean(&self) -> bool {
        self.dirty_files == 0 && !self.merging && !self.rebasing
    }
}

// ============================================================================
// Branch relationship (how two refs relate to each other)
// ============================================================================

/// The relationship between two commits in the same repo
#[derive(Debug, Clone, PartialEq)]
pub enum Ancestry {
    /// A and B are the same commit
    Same,
    /// A is an ancestor of B (B is ahead)
    AncestorOf { ahead_count: u32 },
    /// B is an ancestor of A (A is ahead)
    DescendantOf { ahead_count: u32 },
    /// Neither is ancestor — histories diverged
    Diverged {
        a_ahead: u32,
        b_ahead: u32,
        merge_base: Option<CommitHash>,
    },
    /// Can't determine (one commit not known in this repo)
    Unknown,
}

// ============================================================================
// Cross-side state (container × host)
// ============================================================================

/// The combined state of a repo across container and host
#[derive(Debug, Clone)]
pub enum RepoPairState {
    /// Both exist — the interesting case
    BothPresent {
        container: GitRepoState,
        host: HostRepoState,
        /// How container HEAD relates to host target branch
        ancestry: Ancestry,
        /// Content comparison (tree diff, independent of commit history)
        content: ContentComparison,
    },
    /// Only in container
    ContainerOnly {
        container: GitRepoState,
    },
    /// Only on host
    HostOnly {
        host: HostRepoState,
    },
    /// Container has it but host path isn't a git repo
    ContainerWithBrokenHost {
        container: GitRepoState,
        host_path: PathBuf,
    },
    /// Neither side (shouldn't happen, but type completeness)
    Neither,
}

/// Host-side repo state includes branches we care about
#[derive(Debug, Clone)]
pub struct HostRepoState {
    /// Current HEAD
    pub head: CommitHash,
    /// Dirty state
    pub dirty_files: u32,
    /// The session branch (named after the session)
    pub session_branch: Option<BranchState>,
    /// The target branch (e.g. main)
    pub target_branch: Option<BranchState>,
    /// Squash-base ref (tracks last squash-merge point)
    pub squash_base: Option<CommitHash>,
}

/// State of a specific branch
#[derive(Debug, Clone)]
pub struct BranchState {
    pub name: String,
    pub head: CommitHash,
}

// ============================================================================
// Content comparison (tree-level, ignoring history)
// ============================================================================

/// Whether two trees have the same content (independent of commit history)
#[derive(Debug, Clone, PartialEq)]
pub enum ContentComparison {
    /// Trees are identical (git diff --quiet succeeds)
    Identical,
    /// Trees differ
    Different {
        files_changed: u32,
        insertions: u32,
        deletions: u32,
    },
    /// Can't compare (one side's commit not known on host)
    Incomparable,
}

// ============================================================================
// Merge state (what happens when you try to merge)
// ============================================================================

/// Result of attempting (or dry-running) a merge
#[derive(Debug, Clone)]
pub enum MergeOutcome {
    /// Already up to date (target contains source)
    AlreadyUpToDate,
    /// Can fast-forward
    FastForward { commits: u32 },
    /// Can squash-merge (squash-base exists)
    SquashMerge {
        commits: u32,
        squash_base: CommitHash,
    },
    /// Clean 3-way merge possible
    CleanMerge,
    /// Merge would conflict
    Conflict {
        files: Vec<String>,
    },
    /// Would create the target branch (doesn't exist yet)
    CreateBranch { from: CommitHash },
    /// Can't merge (missing branch, dirty state, etc.)
    Blocked(MergeBlocker),
}

/// Why a merge can't proceed
#[derive(Debug, Clone)]
pub enum MergeBlocker {
    /// Host has uncommitted changes
    HostDirty,
    /// No session branch on host (not extracted)
    NoSessionBranch,
    /// No target branch on host
    NoTargetBranch,
    /// Container has uncommitted changes
    ContainerDirty,
    /// Merge already in progress
    MergeInProgress,
    /// Repo not found
    RepoMissing,
}

impl std::fmt::Display for MergeOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyUpToDate => write!(f, "already up to date"),
            Self::FastForward { commits } => write!(f, "fast-forward {} commit(s)", commits),
            Self::SquashMerge { commits, .. } => write!(f, "squash-merge {} commit(s)", commits),
            Self::CleanMerge => write!(f, "merge cleanly"),
            Self::Conflict { files } => write!(f, "conflict ({})", files.join(", ")),
            Self::CreateBranch { from } => write!(f, "create branch from {}", from),
            Self::Blocked(b) => write!(f, "blocked: {}", b),
        }
    }
}

impl std::fmt::Display for MergeBlocker {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HostDirty => write!(f, "host has uncommitted changes"),
            Self::NoSessionBranch => write!(f, "no session branch"),
            Self::NoTargetBranch => write!(f, "no target branch"),
            Self::ContainerDirty => write!(f, "container has uncommitted changes"),
            Self::MergeInProgress => write!(f, "merge in progress"),
            Self::RepoMissing => write!(f, "repo not found"),
        }
    }
}

// ============================================================================
// Squash tracking
// ============================================================================

/// How squash-merge history affects the current state
#[derive(Debug, Clone)]
pub enum SquashState {
    /// No prior squash-merges
    NoPriorSquash,
    /// Squash-base exists and is valid (ancestor of session)
    Active {
        base: CommitHash,
        /// Commits since last squash
        new_commits: u32,
    },
    /// Squash-base exists but is stale (not ancestor of session)
    Stale {
        base: CommitHash,
    },
}

/// Whether "ahead" commits on target are our squash-merges or external work
#[derive(Debug, Clone)]
pub enum TargetAheadKind {
    /// All ahead commits are our squash-merges — nothing to worry about
    AllSquashArtifacts,
    /// Some ahead commits are external (real work from other sources)
    HasExternalWork { external_count: u32 },
    /// No ahead commits
    NotAhead,
}
