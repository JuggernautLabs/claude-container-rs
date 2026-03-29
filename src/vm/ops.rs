//! Ops — the 12 primitive operations + pre/postconditions.
//!
//! Each op is a typed state transition: (RepoVM, Input) → (RepoVM, Output).
//! Preconditions are checked against VM state before dispatch.
//! Postconditions update VM state from the backend result.

use super::state::*;

/// The 12 irreducible operations.
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
    Checkout { repo: String, ref_name: String },
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
    /// Attach to an interactive container (agent session).
    AttachContainer { image: String, env: Vec<(String, String)>, mounts: Vec<Mount> },

    // ── Control ──
    /// User confirmation gate.
    Confirm { message: String },
}

/// Mount specification for container ops.
#[derive(Debug, Clone)]
pub struct Mount {
    pub source: String,
    pub target: String,
    pub read_only: bool,
}

/// Result of executing an op.
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
    /// Container output.
    ContainerOutput { exit_code: i64, stdout: String },
    /// Container exited (AttachContainer).
    ContainerExited { exit_code: i64 },
    /// User confirmed or declined.
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

impl Op {
    /// Check whether this op can execute against the current repo state.
    /// Returns Ok(()) if preconditions are met, Err with reason if not.
    pub fn check_preconditions(&self, vm: &SyncVM) -> Result<(), PreconditionError> {
        match self {
            Op::RefRead { repo, .. } | Op::TreeCompare { repo, .. } |
            Op::AncestryCheck { repo, .. } => {
                // Read ops: repo must exist in VM
                if !vm.repos.contains_key(repo) {
                    return Err(PreconditionError {
                        op: format!("{:?}", self),
                        reason: format!("repo '{}' not in VM state", repo),
                    });
                }
                Ok(())
            }

            Op::RefWrite { side, repo, .. } => {
                let r = vm.repos.get(repo).ok_or_else(|| PreconditionError {
                    op: format!("{:?}", self),
                    reason: format!("repo '{}' not in VM state", repo),
                })?;
                match side {
                    Side::Container => {
                        if !r.container_clean {
                            return Err(PreconditionError {
                                op: "RefWrite(container)".into(),
                                reason: "container is dirty".into(),
                            });
                        }
                    }
                    Side::Host => {
                        if !r.host_clean {
                            return Err(PreconditionError {
                                op: "RefWrite(host)".into(),
                                reason: "host is dirty".into(),
                            });
                        }
                    }
                }
                Ok(())
            }

            Op::Checkout { repo, .. } => {
                let r = vm.repos.get(repo).ok_or_else(|| PreconditionError {
                    op: "Checkout".into(),
                    reason: format!("repo '{}' not in VM state", repo),
                })?;
                if r.host_merge_state != HostMergeState::Clean {
                    return Err(PreconditionError {
                        op: "Checkout".into(),
                        reason: "host has merge in progress".into(),
                    });
                }
                Ok(())
            }

            Op::MergeTrees { repo, .. } => {
                // Pure operation — just needs the repo to exist
                if !vm.repos.contains_key(repo) {
                    return Err(PreconditionError {
                        op: "MergeTrees".into(),
                        reason: format!("repo '{}' not in VM state", repo),
                    });
                }
                Ok(())
            }

            Op::Commit { repo, .. } => {
                let r = vm.repos.get(repo).ok_or_else(|| PreconditionError {
                    op: "Commit".into(),
                    reason: format!("repo '{}' not in VM state", repo),
                })?;
                if r.host_merge_state == HostMergeState::Conflicted {
                    return Err(PreconditionError {
                        op: "Commit".into(),
                        reason: "host has unresolved conflicts".into(),
                    });
                }
                Ok(())
            }

            Op::BundleCreate { repo } => {
                let r = vm.repos.get(repo).ok_or_else(|| PreconditionError {
                    op: "BundleCreate".into(),
                    reason: format!("repo '{}' not in VM state", repo),
                })?;
                if !r.container.is_present() {
                    return Err(PreconditionError {
                        op: "BundleCreate".into(),
                        reason: "container has no repo".into(),
                    });
                }
                Ok(())
            }

            Op::BundleFetch { repo, .. } => {
                if !vm.repos.contains_key(repo) {
                    return Err(PreconditionError {
                        op: "BundleFetch".into(),
                        reason: format!("repo '{}' not in VM state", repo),
                    });
                }
                Ok(())
            }

            // Container and control ops have no VM preconditions
            Op::RunContainer { .. } | Op::AttachContainer { .. } | Op::Confirm { .. } => Ok(()),
        }
    }

    /// Apply postconditions: update VM state from the op result.
    /// Called after the backend successfully executes the op.
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
                // After fetch, the session branch is updated
                if let OpResult::Hash(hash) = result {
                    if let Some(r) = vm.repo_mut(repo) {
                        r.session = RefState::At(hash.clone());
                    }
                }
            }

            Op::Checkout { repo, .. } => {
                if let Some(r) = vm.repo_mut(repo) {
                    r.host_merge_state = HostMergeState::Clean;
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

            Op::RunContainer { .. } => {
                // Container ops may change container state — the caller
                // should re-observe after significant container mutations.
            }

            // Read-only ops don't change state
            Op::RefRead { .. } | Op::TreeCompare { .. } | Op::AncestryCheck { .. } |
            Op::MergeTrees { .. } | Op::BundleCreate { .. } |
            Op::AttachContainer { .. } | Op::Confirm { .. } => {}
        }
    }
}
