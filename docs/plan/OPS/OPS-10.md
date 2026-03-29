---
status: SUPERSEDED by VM epic (docs/plan/VM/VM-1.md)
---

# OPS-10: Migrate Commands to Use Programs

blocked_by: [OPS-7, OPS-8, OPS-9]
unlocks: [OPS-11]

## Scope

Rewrite `cmd_push`, `cmd_pull`, `cmd_sync` to use the new pipeline:

```rust
let plan = engine.observe(session, branch, repos).await?;
let program = SyncProgram::for_<direction>(&plan);
program.render_preview();
if !program.confirm(auto_yes)? { return Ok(()); }
let result = engine.run_program(session, program, repos).await;
result.render();
```

### cmd_push (src/cmd/push.rs)

Before:
```rust
let plan = build_sync_plan(...).await?;
let has_pushes = plan.action.repo_actions.iter()...;
render::sync_plan_directed(&plan.action, "push");
engine.execute_push(name, plan.action, &repo_paths).await?;
render::sync_result(&result);
```

After:
```rust
let plan = engine.plan_sync(name, branch, &repos).await?;
let program = SyncProgram::for_push(&plan.action);
if program.ops.is_empty() {
    println!("Nothing to push.");
    return Ok(());
}
program.render_preview();
if dry_run { return Ok(()); }
if !confirm("Execute push?", auto_yes) { return Ok(()); }
let result = engine.run_program(name, program, &repos).await;
result.render();
```

### cmd_pull (src/cmd/pull.rs)

Pull is the complex one because of the re-plan loop. The program
handles this: `for_pull` emits Extract ops + ReObserve + Merge ops.
The interpreter calls plan_sync again at ReObserve.

But the current pull also has interactive merge confirmation per-repo,
diverged repo choice (auto/skip/reconcile), and conflict reconciliation.
These become Confirm and LaunchReconciliation ops in the program.

For V1, the pull command can generate the program in two halves:
1. First program: extracts only
2. After extraction: re-plan, generate second program: merges only
3. This matches the current flow without needing the interpreter to
   handle ReObserve regeneration

### cmd_sync (src/cmd/sync_cmd.rs)

Before:
```rust
let plan = build_sync_plan(...).await?;
render::sync_plan_directed(&plan.action, "sync");
engine.execute_sync(name, plan.action, &repo_paths).await?;
```

After:
```rust
let plan = engine.plan_sync(name, branch, &repos).await?;
let program = SyncProgram::for_sync(&plan.action);
program.render_preview();
if dry_run || program.ops.is_empty() { return Ok(()); }
if !confirm("Execute sync?", auto_yes) { return Ok(()); }
let result = engine.run_program(name, program, &repos).await;
result.render();
```

### build_sync_plan refactor

`build_sync_plan()` in sync_cmd.rs currently does session discovery +
config loading + plan_sync. This stays as-is — it returns the plan,
and the caller wraps it in a program.

## Test

Docker integration tests:

```rust
#[tokio::test]
#[ignore]
async fn cmd_push_uses_program_pipeline() {
    // Setup: container in sync, target ahead
    // cmd_push(name, branch, filter, include_deps, dry_run=false, auto_yes=true)
    // Assert: container HEAD advanced (inject happened)
    // Assert: target HEAD unchanged (no merge)
}
```

## Acceptance criteria

- cmd_push uses SyncProgram::for_push + run_program
- cmd_pull uses SyncProgram::for_pull (or two-phase) + run_program
- cmd_sync uses SyncProgram::for_sync + run_program
- All existing integration tests still pass
- Preview output unchanged (or improved with numbered ops)
- Dry-run shows program without executing
