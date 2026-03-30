//! Interpreter — walks an op tree, dispatches to backend, updates VM state.
//!
//! The interpreter is a recursive tree walker. Primitives dispatch to the
//! backend directly. Compound ops (TryMerge, AgentRun, InteractiveSession)
//! dispatch their inner op, check the result, and follow the appropriate
//! branch (on_clean/on_conflict/on_error, on_success/on_failure, on_exit).

use std::path::Path;
use super::state::*;
use super::ops::*;
use super::backend::*;

/// Result of running a program.
#[derive(Debug)]
pub struct ProgramResult {
    pub outcomes: Vec<StepOutcome>,
    pub halted: bool,
    pub halt_reason: Option<String>,
}

/// One step's outcome.
#[derive(Debug)]
pub struct StepOutcome {
    pub op_description: String,
    pub result: StepResult,
}

#[derive(Debug)]
pub enum StepResult {
    Ok(OpResult),
    PreconditionFailed(String),
    BackendError(String),
    Declined,
}

impl StepResult {
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok(_))
    }
}

impl ProgramResult {
    pub fn succeeded(&self) -> usize {
        self.outcomes.iter().filter(|o| o.result.is_ok()).count()
    }
    pub fn failed(&self) -> usize {
        self.outcomes.iter().filter(|o| !o.result.is_ok()).count()
    }
}

impl SyncVM {
    /// Run a program (list of ops) against a backend.
    /// Walks the op tree recursively. Stops on unrecoverable error or user decline.
    pub fn run(&mut self, backend: &dyn VmBackend, ops: Vec<Op>) -> ProgramResult {
        let mut result = ProgramResult {
            outcomes: Vec::new(),
            halted: false,
            halt_reason: None,
        };
        self.run_ops(backend, &ops, &mut result);
        result
    }

    /// Recursive inner loop.
    fn run_ops(&mut self, backend: &dyn VmBackend, ops: &[Op], result: &mut ProgramResult) {
        for op in ops {
            if result.halted { return; }
            self.run_one(backend, op, result);
        }
    }

    /// Execute a single op (primitive or compound).
    fn run_one(&mut self, backend: &dyn VmBackend, op: &Op, result: &mut ProgramResult) {
        // Check preconditions
        if let Err(e) = op.check_preconditions(self) {
            let reason = e.reason.clone();
            result.outcomes.push(StepOutcome {
                op_description: op_name(op),
                result: StepResult::PreconditionFailed(reason.clone()),
            });
            result.halted = true;
            result.halt_reason = Some(format!("precondition failed: {}", reason));
            return;
        }

        match op {
            // ── Compound: TryMerge ──
            Op::TryMerge { repo, ours, theirs, on_clean, on_conflict, on_error } => {
                let repo_vm = match self.repo(repo) {
                    Some(r) => r,
                    None => return,
                };
                let repo_path = match &repo_vm.host_path {
                    Some(p) => p.clone(),
                    None => return,
                };

                match backend.merge_trees(&repo_path, ours, theirs) {
                    Ok((true, tree, _)) => {
                        // Clean merge
                        let op_result = OpResult::MergeResult {
                            clean: true, tree, conflicts: vec![],
                        };
                        self.record(op.clone(), OpOutcome::Ok);
                        result.outcomes.push(StepOutcome {
                            op_description: format!("TryMerge({}, clean)", repo),
                            result: StepResult::Ok(op_result),
                        });
                        self.run_ops(backend, on_clean, result);
                    }
                    Ok((false, _, conflicts)) => {
                        // Conflict
                        let op_result = OpResult::MergeResult {
                            clean: false, tree: None, conflicts: conflicts.clone(),
                        };
                        self.record(op.clone(), OpOutcome::Conflict(conflicts));
                        result.outcomes.push(StepOutcome {
                            op_description: format!("TryMerge({}, conflict)", repo),
                            result: StepResult::Ok(op_result),
                        });
                        self.run_ops(backend, on_conflict, result);
                    }
                    Err(e) => {
                        self.record(op.clone(), OpOutcome::Failed(e.to_string()));
                        result.outcomes.push(StepOutcome {
                            op_description: format!("TryMerge({}, error)", repo),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        self.run_ops(backend, on_error, result);
                    }
                }
            }

            // ── Compound: AgentRun ──
            Op::AgentRun { repo, task, context, on_success, on_failure } => {
                let mounts = self.agent_mounts(repo);
                match backend.agent_run(task, context, &mounts) {
                    Ok((resolved, description, new_head)) => {
                        let op_result = OpResult::AgentCompleted {
                            resolved, description, new_head: new_head.clone(),
                        };
                        op.apply_postconditions(self, &op_result);

                        if resolved {
                            self.record(op.clone(), OpOutcome::Ok);
                            result.outcomes.push(StepOutcome {
                                op_description: format!("AgentRun({}, resolved)", repo),
                                result: StepResult::Ok(op_result),
                            });
                            self.run_ops(backend, on_success, result);
                        } else {
                            self.record(op.clone(), OpOutcome::Failed("agent did not resolve".into()));
                            result.outcomes.push(StepOutcome {
                                op_description: format!("AgentRun({}, unresolved)", repo),
                                result: StepResult::Ok(op_result),
                            });
                            self.run_ops(backend, on_failure, result);
                        }
                    }
                    Err(e) => {
                        self.record(op.clone(), OpOutcome::Failed(e.to_string()));
                        result.outcomes.push(StepOutcome {
                            op_description: format!("AgentRun({}, error)", repo),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        self.run_ops(backend, on_failure, result);
                    }
                }
            }

            // ── Compound: InteractiveSession ──
            Op::InteractiveSession { prompt, on_exit } => {
                let mounts = vec![]; // TODO: build from VM state
                match backend.interactive_session(prompt.as_deref(), &mounts) {
                    Ok(exit_code) => {
                        let op_result = OpResult::SessionExited { exit_code };
                        op.apply_postconditions(self, &op_result);
                        self.record(op.clone(), OpOutcome::OkWithValue(format!("exit {}", exit_code)));
                        result.outcomes.push(StepOutcome {
                            op_description: "InteractiveSession".into(),
                            result: StepResult::Ok(op_result),
                        });
                        self.run_ops(backend, on_exit, result);
                    }
                    Err(e) => {
                        self.record(op.clone(), OpOutcome::Failed(e.to_string()));
                        result.outcomes.push(StepOutcome {
                            op_description: "InteractiveSession".into(),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        self.run_ops(backend, on_exit, result);
                    }
                }
            }

            // ── Confirm ──
            Op::Confirm { message } => {
                match backend.prompt_user(message) {
                    Ok(true) => {
                        self.record(op.clone(), OpOutcome::Ok);
                        result.outcomes.push(StepOutcome {
                            op_description: format!("Confirm({})", message),
                            result: StepResult::Ok(OpResult::UserDecision(true)),
                        });
                    }
                    Ok(false) => {
                        self.record(op.clone(), OpOutcome::OkWithValue("declined".into()));
                        result.outcomes.push(StepOutcome {
                            op_description: format!("Confirm({})", message),
                            result: StepResult::Declined,
                        });
                        result.halted = true;
                        result.halt_reason = Some("user declined".into());
                    }
                    Err(e) => {
                        result.outcomes.push(StepOutcome {
                            op_description: format!("Confirm({})", message),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        result.halted = true;
                        result.halt_reason = Some(e.to_string());
                    }
                }
            }

            // ── All primitives ──
            primitive => {
                let dispatch_result = dispatch_primitive(backend, self, primitive);
                match dispatch_result {
                    Ok(op_result) => {
                        primitive.apply_postconditions(self, &op_result);
                        self.record(primitive.clone(), OpOutcome::Ok);
                        result.outcomes.push(StepOutcome {
                            op_description: op_name(primitive),
                            result: StepResult::Ok(op_result),
                        });
                    }
                    Err(e) => {
                        self.record(primitive.clone(), OpOutcome::Failed(e.to_string()));
                        result.outcomes.push(StepOutcome {
                            op_description: op_name(primitive),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        result.halted = true;
                        result.halt_reason = Some(format!("{}: {}", op_name(primitive), e));
                    }
                }
            }
        }
    }

    /// Build mount list for agent ops from VM state.
    fn agent_mounts(&self, _repo: &str) -> Vec<Mount> {
        // TODO: build actual mounts from session volumes + host paths
        vec![]
    }
}

/// Dispatch a primitive op to the backend.
fn dispatch_primitive(
    backend: &dyn VmBackend,
    vm: &SyncVM,
    op: &Op,
) -> Result<OpResult, VmBackendError> {
    match op {
        Op::RefRead { repo, ref_name, .. } => {
            let path = repo_path(vm, repo);
            match backend.ref_read(&path, ref_name)? {
                Some(h) => Ok(OpResult::Hash(h)),
                None => Ok(OpResult::Hash(String::new())),
            }
        }
        Op::RefWrite { repo, ref_name, hash, .. } => {
            let path = repo_path(vm, repo);
            backend.ref_write(&path, ref_name, hash)?;
            Ok(OpResult::Unit)
        }
        Op::TreeCompare { repo, a, b } => {
            let path = repo_path(vm, repo);
            let (identical, files) = backend.tree_compare(&path, a, b)?;
            Ok(OpResult::Comparison { identical, files_changed: files })
        }
        Op::AncestryCheck { repo, a, b } => {
            let path = repo_path(vm, repo);
            let ancestry = backend.ancestry_check(&path, a, b)?;
            Ok(OpResult::Ancestry(ancestry))
        }
        Op::MergeTrees { repo, ours, theirs } => {
            let path = repo_path(vm, repo);
            let (clean, tree, conflicts) = backend.merge_trees(&path, ours, theirs)?;
            Ok(OpResult::MergeResult { clean, tree, conflicts })
        }
        Op::Checkout { repo, ref_name, .. } => {
            let path = repo_path(vm, repo);
            backend.checkout(&path, ref_name)?;
            Ok(OpResult::Unit)
        }
        Op::Commit { repo, tree, parents, message } => {
            let path = repo_path(vm, repo);
            let hash = backend.commit(&path, tree, parents, message)?;
            Ok(OpResult::Hash(hash))
        }
        Op::BundleCreate { repo } => {
            let bundle_path = backend.bundle_create(&vm.session_name, repo)?;
            Ok(OpResult::Hash(bundle_path))
        }
        Op::BundleFetch { repo, bundle_path } => {
            let path = repo_path(vm, repo);
            let hash = backend.bundle_fetch(&path, bundle_path)?;
            Ok(OpResult::Hash(hash))
        }
        Op::RunContainer { image, script, mounts } => {
            let (exit_code, stdout) = backend.run_container(image, script, mounts)?;
            Ok(OpResult::ContainerOutput { exit_code, stdout })
        }
        // Compounds and Confirm handled by run_one, not here
        _ => Ok(OpResult::Unit),
    }
}

/// Get the host path for a repo from VM state.
fn repo_path(vm: &SyncVM, repo: &str) -> std::path::PathBuf {
    vm.repo(repo)
        .and_then(|r| r.host_path.clone())
        .unwrap_or_else(|| std::path::PathBuf::from(format!("/unknown/{}", repo)))
}

/// Human-readable op name for trace/results.
fn op_name(op: &Op) -> String {
    match op {
        Op::RefRead { repo, ref_name, .. } => format!("RefRead({}, {})", repo, ref_name),
        Op::RefWrite { repo, ref_name, .. } => format!("RefWrite({}, {})", repo, ref_name),
        Op::TreeCompare { repo, .. } => format!("TreeCompare({})", repo),
        Op::AncestryCheck { repo, .. } => format!("AncestryCheck({})", repo),
        Op::MergeTrees { repo, .. } => format!("MergeTrees({})", repo),
        Op::Checkout { repo, .. } => format!("Checkout({})", repo),
        Op::Commit { repo, .. } => format!("Commit({})", repo),
        Op::BundleCreate { repo } => format!("BundleCreate({})", repo),
        Op::BundleFetch { repo, .. } => format!("BundleFetch({})", repo),
        Op::RunContainer { .. } => "RunContainer".into(),
        Op::TryMerge { repo, .. } => format!("TryMerge({})", repo),
        Op::AgentRun { repo, .. } => format!("AgentRun({})", repo),
        Op::InteractiveSession { .. } => "InteractiveSession".into(),
        Op::Confirm { message } => format!("Confirm({})", message),
    }
}
