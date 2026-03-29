---
status: SUPERSEDED by VM epic (docs/plan/VM/VM-1.md)
---

# OPS-7: Interpreter — Execute Vec<Op> with Cleanup-on-Failure

blocked_by: [OPS-6]
unlocks: [OPS-10]

## Design

The interpreter walks a `Vec<Op>` and calls the corresponding
SyncEngine method for each. On failure, it stops (or skips to next
repo, depending on policy) and reports what happened.

```rust
impl SyncEngine {
    pub async fn run_program(
        &self,
        session: &SessionName,
        program: SyncProgram,
        repo_configs: &BTreeMap<String, PathBuf>,
    ) -> ProgramResult {
        let mut results = Vec::new();

        for op in &program.ops {
            let result = self.execute_op(session, op, repo_configs).await;

            match &result {
                OpResult::Declined => {
                    // User said no — stop program
                    results.push(result);
                    break;
                }
                OpResult::Failed { .. } => {
                    // Op failed — compensating action already ran (OPS-3/4/5).
                    // Record failure, continue to next op.
                    results.push(result);
                }
                OpResult::ReObserved { plan } => {
                    // Mid-program re-observation. The remaining ops may need
                    // to be regenerated based on new state.
                    // For now: record result, continue with remaining ops.
                    // OPS-8 handles regeneration.
                    results.push(result);
                }
                _ => {
                    results.push(result);
                }
            }
        }

        ProgramResult {
            session_name: session.clone(),
            results,
        }
    }

    async fn execute_op(
        &self,
        session: &SessionName,
        op: &Op,
        repo_configs: &BTreeMap<String, PathBuf>,
    ) -> OpResult {
        match op {
            Op::Extract { repo } => { ... }
            Op::Inject { repo, branch } => { ... }
            Op::Merge { repo, from_branch, to_branch, squash } => { ... }
            Op::CloneIntoVolume { repo } => { ... }
            Op::MergeIntoVolume { repo, branch } => { ... }
            Op::ReObserve => { ... }
            Op::LaunchReconciliation { repo, conflicts } => { ... }
            Op::Confirm { message } => { ... }
        }
    }
}
```

### ProgramResult

```rust
pub struct ProgramResult {
    pub session_name: SessionName,
    pub results: Vec<OpResult>,
}

impl ProgramResult {
    pub fn succeeded(&self) -> usize { ... }
    pub fn failed(&self) -> usize { ... }
    pub fn was_declined(&self) -> bool { ... }
}
```

### ReObserve handling

`Op::ReObserve` re-snapshots and re-classifies. The result carries the
new `SessionSyncPlan`. For V1, the interpreter records this but doesn't
regenerate remaining ops — the program was pre-generated and runs as-is.

In a future version, the program could be split into phases:
`pre_observe: Vec<Op>` and `post_observe: Fn(Plan) -> Vec<Op>`, where
the second phase is generated after re-observation. But that's
complexity we don't need yet — the current pull re-plan loop just
builds two programs.

## Test

```rust
#[tokio::test]
#[ignore]
async fn interpreter_runs_extract_then_merge() {
    // Setup: container with commits, host repo
    let program = SyncProgram {
        ops: vec![
            Op::Extract { repo: "test".into() },
            Op::Merge { repo: "test".into(), from: "session".into(),
                        to: "main".into(), squash: true },
        ],
        preview: ...,
    };
    let result = engine.run_program(&session, program, &repos).await;
    assert_eq!(result.succeeded(), 2);
}

#[tokio::test]
#[ignore]
async fn interpreter_stops_on_decline() {
    let program = SyncProgram {
        ops: vec![
            Op::Confirm { message: "proceed?".into() },
            Op::Extract { repo: "test".into() },
        ],
        ..
    };
    // With auto_yes=false and stdin providing "n":
    let result = engine.run_program(&session, program, &repos).await;
    assert!(result.was_declined());
    assert_eq!(result.succeeded(), 0);
}
```

## Acceptance criteria

- `run_program()` executes each Op in order
- Failed ops produce OpResult::Failed (not panics)
- Declined Confirm stops program execution
- ProgramResult counts are correct
- Existing merge/extract/inject cleanup (OPS-3/4/5) runs on failure
