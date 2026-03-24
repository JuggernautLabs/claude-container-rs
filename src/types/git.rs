//! Git state types — the (Container, Host) pair and exhaustive matching.
//!
//! Both sides use the same enum. The sync decision is a function of the pair.
//! Every (GitSide, GitSide) combination is handled — the compiler enforces this.

use std::path::PathBuf;
use super::CommitHash;

// ============================================================================
// One side of the pair
// ============================================================================

/// The state of a git repo on ONE side (container or host).
/// Both sides of the pair use this same type.
#[derive(Debug, Clone)]
pub enum GitSide {
    /// Repo exists, has commits, is clean
    Clean {
        head: CommitHash,
    },
    /// Repo exists, has uncommitted changes
    Dirty {
        head: CommitHash,
        dirty_files: u32,
    },
    /// Repo exists, merge in progress
    Merging {
        head: CommitHash,
    },
    /// Repo exists, rebase in progress
    Rebasing {
        head: CommitHash,
    },
    /// Directory exists but not a git repo
    NotARepo {
        path: PathBuf,
    },
    /// Path doesn't exist
    Missing,
}

impl GitSide {
    /// Get the HEAD commit if the repo exists and has one
    pub fn head(&self) -> Option<&CommitHash> {
        match self {
            Self::Clean { head } |
            Self::Dirty { head, .. } |
            Self::Merging { head } |
            Self::Rebasing { head } => Some(head),
            Self::NotARepo { .. } | Self::Missing => None,
        }
    }

    /// Is this side in a state where we can read from it?
    pub fn is_readable(&self) -> bool {
        matches!(self, Self::Clean { .. } | Self::Dirty { .. })
    }

    /// Is this side in a state where we can write to it (merge into)?
    pub fn is_writable(&self) -> bool {
        matches!(self, Self::Clean { .. })
    }

    /// Is the repo present at all?
    pub fn is_present(&self) -> bool {
        !matches!(self, Self::Missing | Self::NotARepo { .. })
    }
}

// ============================================================================
// The pair — this is THE central type
// ============================================================================

/// A repo viewed from both sides simultaneously.
/// Sync decisions are made by matching on this pair.
///
/// The "triple" model: container HEAD, session branch HEAD (host), target branch HEAD (host).
/// `host` is the session branch state. `target_head` and `session_to_target` capture
/// the relationship between session branch and target branch (e.g., main).
#[derive(Debug, Clone)]
pub struct RepoPair {
    pub name: String,
    pub container: GitSide,
    pub host: GitSide,
    /// Additional context when both sides have commits (container vs session)
    pub relation: Option<PairRelation>,
    /// Target branch HEAD on the host (e.g., main). None if no target branch specified.
    pub target_head: Option<CommitHash>,
    /// Session branch → target branch relationship. None if no target or no session branch.
    pub session_to_target: Option<SessionTargetRelation>,
}

/// When both sides have commits, how do they relate?
#[derive(Debug, Clone)]
pub struct PairRelation {
    pub ancestry: Ancestry,
    pub content: ContentComparison,
    pub squash: SquashState,
    pub target_ahead: TargetAheadKind,
}

/// Relationship between session branch and target branch on the host.
/// This is the third leg of the triple: how far ahead/behind/diverged
/// is the session branch relative to the merge target.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionTargetRelation {
    /// How session and target relate in terms of git ancestry.
    /// Uses the same Ancestry enum: "container" means session, "host" means target.
    pub ancestry: Ancestry,
    /// Tree content comparison between session and target.
    pub content: ContentComparison,
}

// ============================================================================
// Branch relationship
// ============================================================================

#[derive(Debug, Clone, PartialEq)]
pub enum Ancestry {
    /// Same commit
    Same,
    /// Container is ancestor of host (host is ahead)
    ContainerBehind { host_ahead: u32 },
    /// Host is ancestor of container (container is ahead)
    ContainerAhead { container_ahead: u32 },
    /// Neither is ancestor
    Diverged {
        container_ahead: u32,
        host_ahead: u32,
        merge_base: Option<CommitHash>,
    },
    /// Can't determine (container commit not known on host)
    Unknown,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ContentComparison {
    Identical,
    Different {
        files_changed: u32,
        insertions: u32,
        deletions: u32,
    },
    Incomparable,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SquashState {
    NoPriorSquash,
    Active { base: CommitHash, new_commits: u32 },
    Stale { base: CommitHash },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TargetAheadKind {
    NotAhead,
    AllSquashArtifacts,
    HasExternalWork { external_count: u32 },
}

// ============================================================================
// Exhaustive sync decision — the match that handles every (GitSide, GitSide)
// ============================================================================

/// What sync should do for this repo pair.
/// Derived from exhaustive matching on (container, host).
#[derive(Debug, Clone, PartialEq)]
pub enum SyncDecision {
    /// Nothing to do
    Skip { reason: SkipReason },
    /// Container → host (extract + merge)
    Pull { commits: u32 },
    /// Host → container (fast-forward)
    Push { commits: u32 },
    /// Both sides changed — merge host into container, then pull back
    Reconcile { container_ahead: u32, host_ahead: u32 },
    /// Container has it, host doesn't — clone out
    CloneToHost,
    /// Host has it, container doesn't — push in
    PushToContainer,
    /// Session branch is ahead of target — merge session → target (no extraction needed)
    MergeToTarget {
        session_ahead: u32,
        /// The session-to-target relation for display/diff purposes
        session_target: SessionTargetRelation,
    },
    /// Can't sync — blocked by something
    Blocked { reason: BlockReason },
}

#[derive(Debug, Clone, PartialEq)]
pub enum SkipReason {
    Identical,
    SquashIdentical,
    ExtractDisabled,
}

impl std::fmt::Display for SkipReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Identical => write!(f, "identical"),
            Self::SquashIdentical => write!(f, "squash-identical"),
            Self::ExtractDisabled => write!(f, "extract disabled"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum BlockReason {
    ContainerDirty(u32),
    HostDirty,
    ContainerMerging,
    ContainerRebasing,
    HostNotARepo(PathBuf),
}

impl std::fmt::Display for BlockReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContainerDirty(n) => write!(f, "{} dirty file(s) in container", n),
            Self::HostDirty => write!(f, "host has uncommitted changes"),
            Self::ContainerMerging => write!(f, "merge in progress in container"),
            Self::ContainerRebasing => write!(f, "rebase in progress in container"),
            Self::HostNotARepo(p) => write!(f, "host path not a git repo: {}", p.display()),
        }
    }
}

impl RepoPair {
    /// THE exhaustive match. Every (container, host) combination is handled.
    /// This is the single source of truth for sync decisions.
    pub fn sync_decision(&self) -> SyncDecision {
        use GitSide::*;

        match (&self.container, &self.host) {
            // --- One or both sides missing/broken ---
            (Missing, Missing) => SyncDecision::Skip { reason: SkipReason::Identical },
            (Missing, _) => SyncDecision::PushToContainer,
            (_, Missing) => SyncDecision::CloneToHost,
            (NotARepo { .. }, _) => SyncDecision::Skip { reason: SkipReason::Identical }, // container has dir but not git
            (_, NotARepo { path }) => SyncDecision::Blocked { reason: BlockReason::HostNotARepo(path.clone()) },

            // --- Container not writable ---
            (Dirty { dirty_files, .. }, _) =>
                SyncDecision::Blocked { reason: BlockReason::ContainerDirty(*dirty_files) },
            (Merging { .. }, _) =>
                SyncDecision::Blocked { reason: BlockReason::ContainerMerging },
            (Rebasing { .. }, _) =>
                SyncDecision::Blocked { reason: BlockReason::ContainerRebasing },

            // --- Host not writable ---
            (_, Dirty { .. }) =>
                SyncDecision::Blocked { reason: BlockReason::HostDirty },
            (_, Merging { .. }) =>
                SyncDecision::Blocked { reason: BlockReason::HostDirty }, // treat host merging as dirty
            (_, Rebasing { .. }) =>
                SyncDecision::Blocked { reason: BlockReason::HostDirty }, // treat host rebasing as dirty

            // --- Both clean — the real sync logic ---
            (Clean { head: c_head }, Clean { head: h_head }) => {
                self.decide_clean_pair(c_head, h_head)
            }
        }
    }

    /// When both sides are clean, use the relation to decide.
    fn decide_clean_pair(&self, c_head: &CommitHash, h_head: &CommitHash) -> SyncDecision {
        // Same commit — trivially identical (but check triple below)
        if c_head == h_head {
            return self.maybe_merge_to_target(SyncDecision::Skip { reason: SkipReason::Identical });
        }

        // Need the relation to decide
        let Some(rel) = &self.relation else {
            // No relation computed — can't determine, treat as pull
            return SyncDecision::Pull { commits: 1 };
        };

        // Content identical despite different history — squash artifact
        if rel.content == ContentComparison::Identical {
            return self.maybe_merge_to_target(SyncDecision::Skip { reason: SkipReason::SquashIdentical });
        }

        match &rel.ancestry {
            Ancestry::Same => self.maybe_merge_to_target(SyncDecision::Skip { reason: SkipReason::Identical }),

            Ancestry::ContainerAhead { container_ahead } =>
                SyncDecision::Pull { commits: *container_ahead },

            Ancestry::ContainerBehind { host_ahead } => {
                // Host ahead — but are the ahead commits just squash artifacts?
                match &rel.target_ahead {
                    TargetAheadKind::AllSquashArtifacts => {
                        // All "ahead" commits are our own squash-merges
                        if rel.content == ContentComparison::Identical {
                            self.maybe_merge_to_target(SyncDecision::Skip { reason: SkipReason::SquashIdentical })
                        } else {
                            SyncDecision::Push { commits: *host_ahead }
                        }
                    }
                    TargetAheadKind::HasExternalWork { .. } =>
                        SyncDecision::Push { commits: *host_ahead },
                    TargetAheadKind::NotAhead =>
                        self.maybe_merge_to_target(SyncDecision::Skip { reason: SkipReason::Identical }),
                }
            }

            Ancestry::Diverged { container_ahead, host_ahead, .. } =>
                SyncDecision::Reconcile {
                    container_ahead: *container_ahead,
                    host_ahead: *host_ahead,
                },

            Ancestry::Unknown =>
                // Container commit not known on host — must be new work
                SyncDecision::Pull { commits: 1 },
        }
    }

    /// If the container-vs-session decision is Skip, check whether session is
    /// ahead of the target branch. If so, upgrade to MergeToTarget.
    fn maybe_merge_to_target(&self, skip_decision: SyncDecision) -> SyncDecision {
        if !matches!(skip_decision, SyncDecision::Skip { .. }) {
            return skip_decision;
        }

        if let Some(ref st_rel) = self.session_to_target {
            // If trees are identical (content same despite different SHAs, e.g. after squash),
            // it's truly up-to-date — don't suggest merge.
            if st_rel.content == ContentComparison::Identical {
                return skip_decision;
            }

            match &st_rel.ancestry {
                // Session is ahead of target — needs merge
                Ancestry::ContainerAhead { container_ahead } if *container_ahead > 0 => {
                    return SyncDecision::MergeToTarget {
                        session_ahead: *container_ahead,
                        session_target: st_rel.clone(),
                    };
                }
                // Session and target diverged — still a merge-to-target situation
                Ancestry::Diverged { .. } => {
                    let session_ahead = match &st_rel.ancestry {
                        Ancestry::Diverged { container_ahead, .. } => *container_ahead,
                        _ => 0,
                    };
                    return SyncDecision::MergeToTarget {
                        session_ahead,
                        session_target: st_rel.clone(),
                    };
                }
                _ => {}
            }
        }

        skip_decision
    }
}

// ============================================================================
// Merge outcome (result of attempting/dry-running a merge)
// ============================================================================

#[derive(Debug, Clone)]
pub enum MergeOutcome {
    AlreadyUpToDate,
    FastForward { commits: u32 },
    SquashMerge { commits: u32, squash_base: CommitHash },
    CleanMerge,
    Conflict { files: Vec<String> },
    CreateBranch { from: CommitHash },
    Blocked(MergeBlocker),
}

#[derive(Debug, Clone)]
pub enum MergeBlocker {
    HostDirty,
    NoSessionBranch,
    NoTargetBranch,
    ContainerDirty,
    MergeInProgress,
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
            Self::RepoMissing => write!(f, "repo missing"),
        }
    }
}
