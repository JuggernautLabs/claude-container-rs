//! Interpreter — walks an op tree, dispatches to backend, updates VM state.

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
    /// Run a program against a backend.
    pub async fn run<B: VmBackend>(&mut self, backend: &B, ops: Vec<Op>) -> ProgramResult {
        let mut result = ProgramResult {
            outcomes: Vec::new(),
            halted: false,
            halt_reason: None,
        };
        self.run_ops(backend, &ops, &mut result).await;
        result
    }

    fn run_ops<'a, B: VmBackend>(&'a mut self, backend: &'a B, ops: &'a [Op], result: &'a mut ProgramResult) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
        Box::pin(async move {
            for op in ops {
                if result.halted { return; }
                self.run_one(backend, op, result).await;
            }
        })
    }

    async fn run_one<B: VmBackend>(&mut self, backend: &B, op: &Op, result: &mut ProgramResult) {
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
            Op::TryMerge { repo, ours, theirs, on_clean, on_conflict, on_error } => {
                let repo_path = repo_path(self, repo);
                match backend.merge_trees(&repo_path, ours, theirs).await {
                    Ok((true, tree, _)) => {
                        let op_result = OpResult::MergeResult { clean: true, tree, conflicts: vec![] };
                        self.record(op.clone(), OpOutcome::Ok);
                        result.outcomes.push(StepOutcome {
                            op_description: format!("TryMerge({}, clean)", repo),
                            result: StepResult::Ok(op_result),
                        });
                        self.run_ops(backend, on_clean, result).await;
                    }
                    Ok((false, _, conflicts)) => {
                        let op_result = OpResult::MergeResult { clean: false, tree: None, conflicts: conflicts.clone() };
                        self.record(op.clone(), OpOutcome::Conflict(conflicts));
                        result.outcomes.push(StepOutcome {
                            op_description: format!("TryMerge({}, conflict)", repo),
                            result: StepResult::Ok(op_result),
                        });
                        self.run_ops(backend, on_conflict, result).await;
                    }
                    Err(e) => {
                        self.record(op.clone(), OpOutcome::Failed(e.to_string()));
                        result.outcomes.push(StepOutcome {
                            op_description: format!("TryMerge({}, error)", repo),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        self.run_ops(backend, on_error, result).await;
                    }
                }
            }

            Op::AgentRun { repo, task, context, on_success, on_failure } => {
                let mounts = vec![];
                match backend.agent_run(task, context, &mounts).await {
                    Ok((resolved, description, new_head)) => {
                        let op_result = OpResult::AgentCompleted { resolved, description, new_head: new_head.clone() };
                        op.apply_postconditions(self, &op_result);
                        if resolved {
                            self.record(op.clone(), OpOutcome::Ok);
                            result.outcomes.push(StepOutcome {
                                op_description: format!("AgentRun({}, resolved)", repo),
                                result: StepResult::Ok(op_result),
                            });
                            self.run_ops(backend, on_success, result).await;
                        } else {
                            self.record(op.clone(), OpOutcome::Failed("agent did not resolve".into()));
                            result.outcomes.push(StepOutcome {
                                op_description: format!("AgentRun({}, unresolved)", repo),
                                result: StepResult::Ok(op_result),
                            });
                            self.run_ops(backend, on_failure, result).await;
                        }
                    }
                    Err(e) => {
                        self.record(op.clone(), OpOutcome::Failed(e.to_string()));
                        result.outcomes.push(StepOutcome {
                            op_description: format!("AgentRun({}, error)", repo),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        self.run_ops(backend, on_failure, result).await;
                    }
                }
            }

            Op::InteractiveSession { prompt, on_exit } => {
                let mounts = vec![];
                match backend.interactive_session(prompt.as_deref(), &mounts).await {
                    Ok(exit_code) => {
                        let op_result = OpResult::SessionExited { exit_code };
                        op.apply_postconditions(self, &op_result);
                        self.record(op.clone(), OpOutcome::OkWithValue(format!("exit {}", exit_code)));
                        result.outcomes.push(StepOutcome {
                            op_description: "InteractiveSession".into(),
                            result: StepResult::Ok(op_result),
                        });
                        self.run_ops(backend, on_exit, result).await;
                    }
                    Err(e) => {
                        self.record(op.clone(), OpOutcome::Failed(e.to_string()));
                        result.outcomes.push(StepOutcome {
                            op_description: "InteractiveSession".into(),
                            result: StepResult::BackendError(e.to_string()),
                        });
                        self.run_ops(backend, on_exit, result).await;
                    }
                }
            }

            Op::Confirm { message } => {
                match backend.prompt_user(message).await {
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

            primitive => {
                let dispatch_result = dispatch_primitive(backend, self, primitive).await;
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
}

async fn dispatch_primitive<B: VmBackend>(
    backend: &B,
    vm: &SyncVM,
    op: &Op,
) -> Result<OpResult, VmBackendError> {
    match op {
        Op::RefRead { repo, ref_name, .. } => {
            let path = repo_path(vm, repo);
            match backend.ref_read(&path, ref_name).await? {
                Some(h) => Ok(OpResult::Hash(h)),
                None => Ok(OpResult::Hash(String::new())),
            }
        }
        Op::RefWrite { repo, ref_name, hash, .. } => {
            let path = repo_path(vm, repo);
            backend.ref_write(&path, ref_name, hash).await?;
            Ok(OpResult::Unit)
        }
        Op::TreeCompare { repo, a, b } => {
            let path = repo_path(vm, repo);
            let (identical, files) = backend.tree_compare(&path, a, b).await?;
            Ok(OpResult::Comparison { identical, files_changed: files })
        }
        Op::AncestryCheck { repo, a, b } => {
            let path = repo_path(vm, repo);
            let ancestry = backend.ancestry_check(&path, a, b).await?;
            Ok(OpResult::Ancestry(ancestry))
        }
        Op::MergeTrees { repo, ours, theirs } => {
            let path = repo_path(vm, repo);
            let (clean, tree, conflicts) = backend.merge_trees(&path, ours, theirs).await?;
            Ok(OpResult::MergeResult { clean, tree, conflicts })
        }
        Op::Checkout { repo, ref_name, .. } => {
            let path = repo_path(vm, repo);
            backend.checkout(&path, ref_name).await?;
            Ok(OpResult::Unit)
        }
        Op::Commit { repo, tree, parents, message } => {
            let path = repo_path(vm, repo);
            let hash = backend.commit(&path, tree, parents, message).await?;
            Ok(OpResult::Hash(hash))
        }
        Op::BundleCreate { repo } => {
            let bundle_path = backend.bundle_create(&vm.session_name, repo).await?;
            Ok(OpResult::Hash(bundle_path))
        }
        Op::BundleFetch { repo, bundle_path } => {
            let path = repo_path(vm, repo);
            let hash = backend.bundle_fetch(&path, bundle_path).await?;
            Ok(OpResult::Hash(hash))
        }
        Op::RunContainer { image, script, mounts } => {
            let (exit_code, stdout) = backend.run_container(image, script, mounts).await?;
            Ok(OpResult::ContainerOutput { exit_code, stdout })
        }
        Op::Extract { repo, session_branch } => {
            let host_path = repo_path(vm, repo);
            let (commits, new_head) = backend.extract(&vm.session_name, repo, &host_path, session_branch).await?;
            Ok(OpResult::Extracted { commits, new_head })
        }
        Op::Inject { repo, branch } => {
            let host_path = repo_path(vm, repo);
            backend.inject(&vm.session_name, repo, &host_path, branch).await?;
            Ok(OpResult::Injected)
        }
        Op::ForceInject { repo, branch } => {
            let host_path = repo_path(vm, repo);
            backend.force_inject(&vm.session_name, repo, &host_path, branch).await?;
            Ok(OpResult::Injected)
        }
        _ => Ok(OpResult::Unit),
    }
}

fn repo_path(vm: &SyncVM, repo: &str) -> std::path::PathBuf {
    vm.repo(repo)
        .and_then(|r| r.host_path.clone())
        .unwrap_or_else(|| std::path::PathBuf::from(format!("/unknown/{}", repo)))
}

fn op_name(op: &Op) -> String {
    format!("{}", op)
}
