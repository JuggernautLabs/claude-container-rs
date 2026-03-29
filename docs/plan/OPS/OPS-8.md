---
status: SUPERSEDED by VM epic (docs/plan/VM/VM-1.md)
---

# OPS-8: Program Generators — for_push, for_pull, for_sync

blocked_by: [OPS-6]
unlocks: [OPS-10]

## Design

High-level commands don't construct ops directly. They call a generator
that reads the plan (RepoState per repo) and emits the right sequence.

```rust
impl SyncProgram {
    pub fn for_push(plan: &SessionSyncPlan) -> Self { ... }
    pub fn for_pull(plan: &SessionSyncPlan) -> Self { ... }
    pub fn for_sync(plan: &SessionSyncPlan) -> Self { ... }
}
```

### for_push

Reads `state.push_action()` per repo. Only emits Inject/CloneIntoVolume.

```rust
fn for_push(plan: &SessionSyncPlan) -> Self {
    let mut ops = Vec::new();
    for action in &plan.repo_actions {
        match action.state.push_action() {
            PushAction::Inject { .. } =>
                ops.push(Op::Inject { repo: action.repo_name.clone(),
                                       branch: plan.target_branch.clone() }),
            PushAction::PushToContainer =>
                ops.push(Op::Inject { repo: action.repo_name.clone(),
                                       branch: plan.target_branch.clone() }),
            PushAction::Skip | PushAction::Blocked(_) => {}
        }
    }
    Self { ops, preview: ProgramPreview::from_plan(plan, "push") }
}
```

### for_pull

Reads `state.pull_action()` per repo. Emits Extract+ReObserve+Merge
sequences. Reconcile emits MergeIntoVolume+LaunchReconciliation+Extract+Merge.

```rust
fn for_pull(plan: &SessionSyncPlan) -> Self {
    let mut extract_ops = Vec::new();
    let mut merge_ops = Vec::new();

    for action in &plan.repo_actions {
        match action.state.pull_action() {
            PullAction::Extract { .. } => {
                extract_ops.push(Op::Extract { repo: action.repo_name.clone() });
                merge_ops.push(Op::Merge {
                    repo: action.repo_name.clone(),
                    from_branch: plan.session_name.to_string(),
                    to_branch: plan.target_branch.clone(),
                    squash: true,
                });
            }
            PullAction::CloneToHost => {
                extract_ops.push(Op::CloneIntoVolume {
                    repo: action.repo_name.clone(),
                });
            }
            PullAction::MergeToTarget { .. } => {
                merge_ops.push(Op::Merge {
                    repo: action.repo_name.clone(),
                    from_branch: plan.session_name.to_string(),
                    to_branch: plan.target_branch.clone(),
                    squash: true,
                });
            }
            PullAction::Reconcile => {
                // Reconcile needs special handling — may require agent
                let has_conflict = action.trial_conflicts
                    .as_ref().map_or(false, |f| !f.is_empty());
                if has_conflict {
                    let conflicts = action.trial_conflicts.clone().unwrap_or_default();
                    extract_ops.push(Op::MergeIntoVolume {
                        repo: action.repo_name.clone(),
                        branch: plan.target_branch.clone(),
                    });
                    extract_ops.push(Op::LaunchReconciliation {
                        repo: action.repo_name.clone(),
                        conflicts,
                    });
                    merge_ops.push(Op::Extract { repo: action.repo_name.clone() });
                    merge_ops.push(Op::Merge {
                        repo: action.repo_name.clone(),
                        from_branch: plan.session_name.to_string(),
                        to_branch: plan.target_branch.clone(),
                        squash: true,
                    });
                } else {
                    // Auto-reconcile: inject then extract+merge
                    extract_ops.push(Op::Inject {
                        repo: action.repo_name.clone(),
                        branch: plan.target_branch.clone(),
                    });
                    extract_ops.push(Op::Extract { repo: action.repo_name.clone() });
                    merge_ops.push(Op::Merge {
                        repo: action.repo_name.clone(),
                        from_branch: plan.session_name.to_string(),
                        to_branch: plan.target_branch.clone(),
                        squash: true,
                    });
                }
            }
            PullAction::Skip | PullAction::Blocked(_) => {}
        }
    }

    // Assemble: extracts → re-observe → merges
    let mut ops = extract_ops;
    if !merge_ops.is_empty() {
        ops.push(Op::ReObserve);
        ops.extend(merge_ops);
    }
    Self { ops, preview: ProgramPreview::from_plan(plan, "pull") }
}
```

### for_sync

Push phase first, then pull phase:
```rust
fn for_sync(plan: &SessionSyncPlan) -> Self {
    let push = Self::for_push(plan);
    let pull = Self::for_pull(plan);
    let mut ops = push.ops;
    ops.extend(pull.ops);
    Self { ops, preview: ProgramPreview::from_plan(plan, "sync") }
}
```

## Test

All pure tests — no Docker needed. Construct a SessionSyncPlan with
known RepoStates, call the generator, assert on the emitted ops.

```rust
#[test]
fn for_push_emits_inject_for_target_ahead() {
    let plan = make_plan(vec![
        repo_action("alpha", RepoState {
            extraction: LegState::InSync,
            merge: MergeLeg::TargetAhead { commits: 3, all_squash: false },
            blocker: None,
        }),
    ]);
    let program = SyncProgram::for_push(&plan);
    assert_eq!(program.ops.len(), 1);
    assert!(matches!(program.ops[0], Op::Inject { .. }));
}

#[test]
fn for_pull_emits_extract_reobserve_merge() {
    let plan = make_plan(vec![
        repo_action("alpha", RepoState {
            extraction: LegState::ContainerAhead { commits: 5 },
            merge: MergeLeg::InSync,
            blocker: None,
        }),
    ]);
    let program = SyncProgram::for_pull(&plan);
    assert_eq!(program.ops.len(), 3);
    assert!(matches!(program.ops[0], Op::Extract { .. }));
    assert!(matches!(program.ops[1], Op::ReObserve));
    assert!(matches!(program.ops[2], Op::Merge { .. }));
}

#[test]
fn for_push_skips_pull_only_repos() {
    let plan = make_plan(vec![
        repo_action("alpha", RepoState {
            extraction: LegState::ContainerAhead { commits: 5 },
            merge: MergeLeg::InSync,
            blocker: None,
        }),
    ]);
    let program = SyncProgram::for_push(&plan);
    assert!(program.ops.is_empty());
}

#[test]
fn for_sync_push_before_pull() {
    let plan = make_plan(vec![
        repo_action("alpha", RepoState {
            extraction: LegState::ContainerAhead { commits: 3 },
            merge: MergeLeg::TargetAhead { commits: 2, all_squash: false },
            blocker: None,
        }),
    ]);
    let program = SyncProgram::for_sync(&plan);
    // First op should be inject (push phase)
    assert!(matches!(program.ops[0], Op::Inject { .. }));
    // Then extract (pull phase)
    assert!(matches!(program.ops[1], Op::Extract { .. }));
}
```

## Acceptance criteria

- for_push only emits Inject ops
- for_pull emits Extract/ReObserve/Merge in correct order
- for_sync puts push ops before pull ops
- Blocked and Skip repos produce no ops
- Reconcile with conflicts emits MergeIntoVolume + LaunchReconciliation
- Reconcile without conflicts emits Inject + Extract + Merge
- All tests run without Docker
