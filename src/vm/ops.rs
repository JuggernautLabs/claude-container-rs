//! Ops — primitives + compound ops with success/failure paths.
//!
//! Primitives are irreducible git/docker/control operations.
//! Compounds carry their own branching and cleanup — no external
//! if/else needed. The interpreter walks the tree recursively.
//!
//! Design philosophy: agents and humans are first-class ops,
//! not special cases. Every op that can fail carries its cleanup.

use super::state::*;

// ============================================================================
// Primitives — the 12 irreducible operations
// ============================================================================

/// An operation in the sync VM.
#[derive(Debug, Clone)]
pub enum Op {
    // ── Ref ops ──
    /// Read a git ref on either side.
    RefRead { side: Side, repo: String, ref_name: String },
    /// Write a git ref on either side.
    RefWrite { side: Side, repo: String, ref_name: String, hash: String },

    // ── Tree ops ──
    /// Compare two tree objects (content diff).
    TreeCompare { repo: String, a: String, b: String },
    /// Check ancestry between two commits.
    AncestryCheck { repo: String, a: String, b: String },
    /// In-memory merge of two trees (no side effects).
    MergeTrees { repo: String, ours: String, theirs: String },
    /// Update worktree to match a ref.
    Checkout { side: Side, repo: String, ref_name: String },
    /// Create a commit object.
    Commit { repo: String, tree: String, parents: Vec<String>, message: String },

    // ── Transport ops ──
    /// Create a git bundle in the container.
    BundleCreate { repo: String },
    /// Fetch a bundle on the host.
    BundleFetch { repo: String, bundle_path: String },

    // ── Container ops ──
    /// Run a throwaway container (script, collect output, remove).
    RunContainer { image: String, script: String, mounts: Vec<Mount> },

    // ── Control ──
    /// User confirmation gate.
    Confirm { message: String },

    // ============================================================================
    // Compound ops — carry success/failure/cleanup paths
    // ============================================================================

    /// Try a merge. Branch on the result.
    /// The interpreter runs MergeTrees, then follows the appropriate path.
    TryMerge {
        repo: String,
        ours: String,
        theirs: String,
        on_clean: Vec<Op>,
        on_conflict: Vec<Op>,
        on_error: Vec<Op>,
    },

    /// Run an agent (Claude) with a specific task.
    /// Agent signals completion via `fin`. VM reads structured result.
    AgentRun {
        repo: String,
        task: AgentTask,
        context: String,
        on_success: Vec<Op>,
        on_failure: Vec<Op>,
    },

    /// Drop a human into the environment.
    /// On exit, state is unknown — must re-observe.
    InteractiveSession {
        prompt: Option<String>,
        on_exit: Vec<Op>,
    },
}

/// What an agent is asked to do.
#[derive(Debug, Clone)]
pub enum AgentTask {
    /// Resolve merge conflicts (markers in worktree).
    ResolveConflicts { files: Vec<String> },
    /// General work session.
    Work,
    /// Headless run — non-interactive, output captured.
    Run { prompt: String },
    /// Review session — read-only.
    Review { prompt: String },
}

/// Mount specification for container ops.
#[derive(Debug, Clone)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

// ============================================================================
// Results
// ============================================================================

/// Result of executing a primitive op.
#[derive(Debug, Clone)]
pub enum OpResult {
    /// A hash value (from RefRead, BundleFetch, Commit).
    Hash(String),
    /// Content comparison result.
    Comparison { identical: bool, files_changed: u32 },
    /// Ancestry result.
    Ancestry(AncestryResult),
    /// Merge result (from MergeTrees).
    MergeResult { clean: bool, tree: Option<String>, conflicts: Vec<String> },
    /// Container output (from RunContainer).
    ContainerOutput { exit_code: i64, stdout: String },
    /// Agent completed (from AgentRun).
    AgentCompleted { resolved: bool, description: Option<String>, new_head: Option<String> },
    /// Human exited (from InteractiveSession).
    SessionExited { exit_code: i64 },
    /// User confirmed or declined (from Confirm).
    UserDecision(bool),
    /// No meaningful output (RefWrite, Checkout, etc.).
    Unit,
}

/// Ancestry relationship between two commits.
#[derive(Debug, Clone, PartialEq)]
pub enum AncestryResult {
    Same,
    AIsAncestorOfB { distance: u32 },
    BIsAncestorOfA { distance: u32 },
    Diverged { a_ahead: u32, b_ahead: u32, merge_base: Option<String> },
    Unknown,
}

// ============================================================================
// Precondition / postcondition errors
// ============================================================================

/// Precondition error — op cannot execute in current state.
#[derive(Debug, Clone)]
pub struct PreconditionError {
    pub op: String,
    pub reason: String,
}

impl std::fmt::Display for PreconditionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.op, self.reason)
    }
}

impl std::error::Error for PreconditionError {}

// ============================================================================
// Preconditions — checked before dispatch
// ============================================================================

impl Op {
    /// Check whether this op can execute against the current VM state.
    pub fn check_preconditions(&self, vm: &SyncVM) -> Result<(), PreconditionError> {
        match self {
            // Read ops: repo must exist in VM
            Op::RefRead { repo, .. } | Op::TreeCompare { repo, .. } |
            Op::AncestryCheck { repo, .. } | Op::MergeTrees { repo, .. } => {
                require_repo(vm, repo, self)
            }

            Op::RefWrite { side, repo, .. } => {
                require_repo(vm, repo, self)?;
                let r = vm.repo(repo).unwrap();
                match side {
                    Side::Container if !r.container_clean =>
                        Err(precondition_err(self, "container is dirty")),
                    _ => Ok(()),
                }
            }

            Op::Checkout { side, repo, .. } => {
                require_repo(vm, repo, self)?;
                let r = vm.repo(repo).unwrap();
                match side {
                    Side::Host if r.host_merge_state != HostMergeState::Clean =>
                        Err(precondition_err(self, "host has merge in progress")),
                    _ => Ok(()),
                }
            }

            Op::Commit { repo, .. } => {
                require_repo(vm, repo, self)?;
                let r = vm.repo(repo).unwrap();
                if r.host_merge_state == HostMergeState::Conflicted {
                    Err(precondition_err(self, "host has unresolved conflicts"))
                } else {
                    Ok(())
                }
            }

            Op::BundleCreate { repo } => {
                require_repo(vm, repo, self)?;
                let r = vm.repo(repo).unwrap();
                if !r.container.is_present() {
                    Err(precondition_err(self, "no container repo"))
                } else {
                    Ok(())
                }
            }

            Op::BundleFetch { repo, .. } => require_repo(vm, repo, self),

            // Compounds delegate to their first op's preconditions
            Op::TryMerge { repo, .. } => require_repo(vm, repo, self),
            Op::AgentRun { repo, .. } => require_repo(vm, repo, self),

            // No VM preconditions
            Op::RunContainer { .. } | Op::Confirm { .. } |
            Op::InteractiveSession { .. } => Ok(()),
        }
    }
}

fn require_repo(vm: &SyncVM, repo: &str, op: &Op) -> Result<(), PreconditionError> {
    if vm.repos.contains_key(repo) {
        Ok(())
    } else {
        Err(precondition_err(op, &format!("repo '{}' not in VM state", repo)))
    }
}

fn precondition_err(op: &Op, reason: &str) -> PreconditionError {
    let op_name = match op {
        Op::RefRead { .. } => "RefRead",
        Op::RefWrite { .. } => "RefWrite",
        Op::TreeCompare { .. } => "TreeCompare",
        Op::AncestryCheck { .. } => "AncestryCheck",
        Op::MergeTrees { .. } => "MergeTrees",
        Op::Checkout { .. } => "Checkout",
        Op::Commit { .. } => "Commit",
        Op::BundleCreate { .. } => "BundleCreate",
        Op::BundleFetch { .. } => "BundleFetch",
        Op::RunContainer { .. } => "RunContainer",
        Op::TryMerge { .. } => "TryMerge",
        Op::AgentRun { .. } => "AgentRun",
        Op::InteractiveSession { .. } => "InteractiveSession",
        Op::Confirm { .. } => "Confirm",
    };
    PreconditionError { op: op_name.to_string(), reason: reason.to_string() }
}

// ============================================================================
// Postconditions — update VM state from result
// ============================================================================

impl Op {
    /// Update VM state after successful execution.
    pub fn apply_postconditions(&self, vm: &mut SyncVM, result: &OpResult) {
        let session_name = vm.session_name.clone();
        match self {
            Op::RefWrite { side, repo, ref_name, hash } => {
                if let Some(r) = vm.repo_mut(repo) {
                    let new_ref = RefState::At(hash.clone());
                    match side {
                        Side::Container => r.container = new_ref,
                        Side::Host => {
                            if ref_name.contains(&session_name) {
                                r.session = new_ref;
                            } else {
                                r.target = new_ref;
                            }
                        }
                    }
                }
            }

            Op::BundleFetch { repo, .. } => {
                if let OpResult::Hash(hash) = result {
                    if let Some(r) = vm.repo_mut(repo) {
                        r.session = RefState::At(hash.clone());
                    }
                }
            }

            Op::Checkout { side, repo, .. } => {
                if let Some(r) = vm.repo_mut(repo) {
                    match side {
                        Side::Host => r.host_merge_state = HostMergeState::Clean,
                        Side::Container => r.conflict = ConflictState::Clean,
                    }
                }
            }

            Op::Commit { repo, .. } => {
                if let OpResult::Hash(hash) = result {
                    if let Some(r) = vm.repo_mut(repo) {
                        r.target = RefState::At(hash.clone());
                        r.host_merge_state = HostMergeState::Clean;
                    }
                }
            }

            Op::AgentRun { repo, .. } => {
                if let OpResult::AgentCompleted { resolved, new_head, .. } = result {
                    if let Some(r) = vm.repo_mut(repo) {
                        if *resolved {
                            r.conflict = ConflictState::Resolved;
                            if let Some(hash) = new_head {
                                r.container = RefState::At(hash.clone());
                            }
                        }
                    }
                }
            }

            Op::InteractiveSession { .. } => {
                // Human was in the container — all state is unknown.
                // Caller should re-observe after this op.
                for (_, repo) in vm.repos.iter_mut() {
                    repo.container = RefState::Absent; // force re-observe
                }
            }

            // Read-only or container ops don't change tracked state
            Op::RefRead { .. } | Op::TreeCompare { .. } | Op::AncestryCheck { .. } |
            Op::MergeTrees { .. } | Op::BundleCreate { .. } |
            Op::RunContainer { .. } | Op::Confirm { .. } |
            Op::TryMerge { .. } => {}
        }
    }
}

// ============================================================================
// Builder helpers — convenience constructors for common ops
// ============================================================================

impl Op {
    pub fn ref_read(side: Side, repo: &str, ref_name: &str) -> Self {
        Op::RefRead { side, repo: repo.into(), ref_name: ref_name.into() }
    }
    pub fn ref_write(side: Side, repo: &str, ref_name: &str, hash: &str) -> Self {
        Op::RefWrite { side, repo: repo.into(), ref_name: ref_name.into(), hash: hash.into() }
    }
    pub fn checkout(side: Side, repo: &str, ref_name: &str) -> Self {
        Op::Checkout { side, repo: repo.into(), ref_name: ref_name.into() }
    }
    pub fn commit(repo: &str, tree: &str, parents: &[&str], msg: &str) -> Self {
        Op::Commit {
            repo: repo.into(),
            tree: tree.into(),
            parents: parents.iter().map(|s| s.to_string()).collect(),
            message: msg.into(),
        }
    }
    pub fn bundle_create(repo: &str) -> Self {
        Op::BundleCreate { repo: repo.into() }
    }
    pub fn bundle_fetch(repo: &str, path: &str) -> Self {
        Op::BundleFetch { repo: repo.into(), bundle_path: path.into() }
    }
    pub fn confirm(msg: &str) -> Self {
        Op::Confirm { message: msg.into() }
    }
}
