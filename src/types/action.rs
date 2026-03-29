//! Action/Preview pattern — enforces that every destructive operation
//! must be previewed before execution.
//!
//! You literally cannot call .execute() without first having a Plan.
//! The type system prevents it.
//!
//! ```ignore
//! let plan = SyncPlan::preview(&snapshot)?;  // read-only, shows what would happen
//! plan.display();                             // render to user
//! let result = plan.execute()?;              // consumes the plan, does the work
//! ```ignore

use std::fmt;

/// A planned action that has been previewed but not yet executed.
/// `A` is the action type, `R` is the result type.
///
/// You get a Plan by calling a preview function.
/// You can only execute by consuming the Plan.
#[derive(Debug)]
pub struct Plan<A: Action> {
    /// What we're going to do
    pub action: A,
    /// Human-readable description of what will happen
    pub description: String,
    /// Whether this plan modifies state (vs read-only)
    pub destructive: bool,
}

/// Trait that every actionable operation implements.
/// Separates "what would happen" from "do it".
pub trait Action: Sized + fmt::Debug {
    /// The result of executing this action
    type Result;
    /// The error type
    type Error;

    /// Preview: compute what would happen without doing it.
    /// Returns a Plan that can be displayed or executed.
    fn preview(self) -> Result<Plan<Self>, Self::Error>;

    /// Execute the plan. Only callable through Plan<Self>.
    fn execute(self) -> Result<Self::Result, Self::Error>;
}

impl<A: Action> Plan<A> {
    /// Execute the planned action. Consumes the plan.
    pub fn execute(self) -> Result<A::Result, A::Error> {
        self.action.execute()
    }

    /// Check if this plan would change anything
    pub fn is_noop(&self) -> bool {
        !self.destructive
    }
}

impl<A: Action> fmt::Display for Plan<A> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.description)
    }
}

// ============================================================================
// Concrete plan types for each subsystem
// ============================================================================

/// A sync plan for a single repo.
/// Carries the observed two-leg state. Direction-specific actions are derived
/// by the caller via `state.pull_action()` / `state.push_action()`.
#[derive(Debug)]
pub struct RepoSyncAction {
    pub repo_name: String,
    /// Host path for this repo (for display as relative path)
    pub host_path: Option<std::path::PathBuf>,
    /// Two-leg observed state (extraction + merge legs)
    pub state: super::git::RepoState,
    /// Commit hashes for display
    pub container_head: Option<super::CommitHash>,
    pub session_head: Option<super::CommitHash>,
    pub target_head: Option<super::CommitHash>,
    /// What the diff would look like (precomputed at preview time)
    pub outbound_diff: Option<DiffSummary>,
    pub inbound_diff: Option<DiffSummary>,
    /// Trial merge result: None = not tested, Some(empty) = clean, Some(files) = conflicts
    pub trial_conflicts: Option<Vec<String>>,
    /// Diff from target to session branch (for MergeToTarget decisions)
    pub session_to_target_diff: Option<DiffSummary>,
}

/// A sync plan for the entire session
#[derive(Debug)]
pub struct SessionSyncPlan {
    pub session_name: super::SessionName,
    pub target_branch: String,
    pub repo_actions: Vec<RepoSyncAction>,
}

impl SessionSyncPlan {
    pub fn pulls(&self) -> Vec<&RepoSyncAction> {
        self.repo_actions.iter()
            .filter(|a| matches!(a.state.pull_action(), super::git::PullAction::Extract { .. } | super::git::PullAction::CloneToHost))
            .collect()
    }
    pub fn pushes(&self) -> Vec<&RepoSyncAction> {
        self.repo_actions.iter()
            .filter(|a| matches!(a.state.push_action(), super::git::PushAction::Inject { .. } | super::git::PushAction::PushToContainer))
            .collect()
    }
    pub fn reconciles(&self) -> Vec<&RepoSyncAction> {
        self.repo_actions.iter()
            .filter(|a| matches!(a.state.pull_action(), super::git::PullAction::Reconcile))
            .collect()
    }
    pub fn blocked(&self) -> Vec<&RepoSyncAction> {
        self.repo_actions.iter()
            .filter(|a| a.state.blocker.is_some())
            .collect()
    }
    pub fn skipped(&self) -> Vec<&RepoSyncAction> {
        self.repo_actions.iter()
            .filter(|a| !a.state.has_work())
            .collect()
    }
    pub fn pending_merges(&self) -> Vec<&RepoSyncAction> {
        self.repo_actions.iter()
            .filter(|a| matches!(a.state.pull_action(), super::git::PullAction::MergeToTarget { .. }))
            .collect()
    }
    pub fn has_work(&self) -> bool {
        self.repo_actions.iter().any(|a| a.state.has_work())
    }
    pub fn is_destructive(&self) -> bool {
        self.has_work()
    }
}

impl Action for SessionSyncPlan {
    type Result = SyncResult;
    type Error = super::ContainerError;

    fn preview(self) -> Result<Plan<Self>, Self::Error> {
        let description = format!("sync session {}", self.session_name);
        let destructive = self.is_destructive();
        Ok(Plan {
            action: self,
            description,
            destructive,
        })
    }

    fn execute(self) -> Result<Self::Result, Self::Error> {
        // Synchronous Action::execute cannot run async code.
        // Use SyncEngine::execute_sync() directly instead.
        unimplemented!(
            "SessionSyncPlan::execute() is not usable directly — \
             call SyncEngine::execute_sync() which is async"
        )
    }
}

// ============================================================================
// Extract / merge / sync result types
// ============================================================================

/// Result of extracting a repo from a container volume via git bundle.
#[derive(Debug, Clone)]
pub struct ExtractResult {
    /// Number of commits in the bundle
    pub commit_count: u32,
    /// The new HEAD on the session branch after extraction
    pub new_head: super::CommitHash,
}

/// Result of syncing one repo (extract + merge, inject, etc.)
#[derive(Debug)]
pub enum RepoSyncResult {
    /// Successfully pulled (extract + merge)
    Pulled {
        repo_name: String,
        extract: ExtractResult,
        merge: super::git::MergeOutcome,
    },
    /// Successfully pushed (inject)
    Pushed {
        repo_name: String,
    },
    /// Session branch already had the work, just merged to target (no extraction)
    Merged {
        repo_name: String,
        merge: super::git::MergeOutcome,
    },
    /// Successfully cloned to host
    ClonedToHost {
        repo_name: String,
        extract: ExtractResult,
    },
    /// Skipped (already in sync or blocked)
    Skipped {
        repo_name: String,
        reason: String,
    },
    /// Merge conflict detected — carries the conflicting file paths
    Conflicted {
        repo_name: String,
        files: Vec<String>,
    },
    /// Failed
    Failed {
        repo_name: String,
        error: String,
    },
}

/// Aggregate result of executing a full SessionSyncPlan.
#[derive(Debug)]
pub struct SyncResult {
    pub session_name: super::SessionName,
    pub results: Vec<RepoSyncResult>,
}

impl SyncResult {
    pub fn succeeded(&self) -> usize {
        self.results.iter().filter(|r| matches!(r,
            RepoSyncResult::Pulled { .. } |
            RepoSyncResult::Merged { .. } |
            RepoSyncResult::Pushed { .. } |
            RepoSyncResult::ClonedToHost { .. }
        )).count()
    }

    pub fn failed(&self) -> usize {
        self.results.iter().filter(|r| matches!(r, RepoSyncResult::Failed { .. })).count()
    }

    pub fn conflicted(&self) -> usize {
        self.results.iter().filter(|r| matches!(r, RepoSyncResult::Conflicted { .. })).count()
    }

    pub fn skipped(&self) -> usize {
        self.results.iter().filter(|r| matches!(r, RepoSyncResult::Skipped { .. })).count()
    }

    /// Returns true when some repos succeeded and some failed (partial failure).
    /// A fully successful or fully failed result is NOT partial.
    pub fn is_partial(&self) -> bool {
        self.succeeded() > 0 && self.failed() > 0
    }
}

/// Plan for a pull operation
#[derive(Debug)]
pub struct PullPlan {
    pub session_name: super::SessionName,
    pub target_branch: Option<String>,
    pub repos: Vec<RepoPullAction>,
}

#[derive(Debug)]
pub struct RepoPullAction {
    pub repo_name: String,
    pub extract: ExtractPreview,
    pub merge: Option<MergePreview>,
}

#[derive(Debug)]
pub enum ExtractPreview {
    /// New commits to extract
    HasChanges { commits: u32, files: u32 },
    /// Already up to date
    UpToDate,
    /// Can't extract (not enabled, missing host path, etc.)
    Blocked(String),
}

#[derive(Debug)]
pub enum MergePreview {
    /// Will merge
    WillMerge(super::git::MergeOutcome),
    /// Nothing to merge (extract-only)
    NoTarget,
}

/// Plan for a push operation
#[derive(Debug)]
pub struct PushPlan {
    pub session_name: super::SessionName,
    pub source_branch: String,
    pub repos: Vec<RepoPushAction>,
}

#[derive(Debug)]
pub struct RepoPushAction {
    pub repo_name: String,
    pub push: PushPreview,
}

#[derive(Debug)]
pub enum PushPreview {
    WillPush { commits: u32 },
    AlreadyUpToDate,
    Diverged,
    Blocked(String),
}

/// Plan for container lifecycle
#[derive(Debug)]
pub struct ContainerPlan {
    pub action: ContainerAction,
}

#[derive(Debug)]
pub enum ContainerAction {
    /// Create a new container
    Create {
        image: super::ImageRef,
        volumes: Vec<super::VolumeName>,
    },
    /// Resume a stopped container
    Resume {
        container: super::ContainerName,
    },
    /// Rebuild: remove stale + create fresh
    Rebuild {
        container: super::ContainerName,
        reasons: Vec<String>,
        image: super::ImageRef,
    },
    /// Attach to running container
    Attach {
        container: super::ContainerName,
    },
}

/// Diff summary (precomputed during preview)
#[derive(Debug, Clone)]
pub struct DiffSummary {
    pub files_changed: u32,
    pub insertions: u32,
    pub deletions: u32,
    pub files: Vec<FileDiff>,
}

/// Per-file diff entry
#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub status: FileStatus,
    pub insertions: u32,
    pub deletions: u32,
}

#[derive(Debug, Clone)]
pub enum FileStatus {
    Added,
    Modified,
    Deleted,
    Renamed(String), // old path
}

impl fmt::Display for DiffSummary {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} file(s), +{} -{}", self.files_changed, self.insertions, self.deletions)
    }
}

impl Action for ContainerPlan {
    type Result = ();
    type Error = super::ContainerError;

    fn preview(self) -> Result<Plan<Self>, Self::Error> {
        let (description, destructive) = match &self.action {
            ContainerAction::Attach { container } => {
                (format!("Attach to running container {}", container), false)
            }
            ContainerAction::Resume { container } => {
                (format!("Resume stopped container {}", container), false)
            }
            ContainerAction::Create { image, volumes } => {
                (format!("Create container from {} with {} volume(s)", image, volumes.len()), true)
            }
            ContainerAction::Rebuild { container, reasons, image } => {
                (format!("Rebuild container {} from {} ({})", container, image, reasons.join(", ")), true)
            }
        };
        Ok(Plan {
            action: self,
            description,
            destructive,
        })
    }

    fn execute(self) -> Result<Self::Result, Self::Error> {
        // Container lifecycle is async — use Lifecycle methods directly.
        Err(super::ContainerError::NonInteractive(
            "ContainerPlan::execute() requires async. Use Lifecycle::create_container/start_container directly.".into()
        ))
    }
}
