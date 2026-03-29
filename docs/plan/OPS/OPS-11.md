---
status: SUPERSEDED by VM epic (docs/plan/VM/VM-1.md)
---

# OPS-11: Remove Old Dispatch + Dead Code

blocked_by: [OPS-10]
unlocks: []

## Scope

Delete everything replaced by the program pipeline:

### From src/sync/mod.rs

- `execute_sync()` — replaced by `run_program()` with `for_sync`
- `execute_push()` — replaced by `run_program()` with `for_push`
- `dispatch_pull()` — replaced by interpreter's `execute_op(Op::Extract/Merge/...)`
- `dispatch_push()` — replaced by interpreter's `execute_op(Op::Inject/...)`
- `execute_pull_one()` — inlined into interpreter Extract+Merge handling

Keep:
- `plan_sync()` — still the observation entry point
- `extract()`, `inject()`, `merge()`, `clone_into_volume()`, `merge_into_volume()` — the atomic ops, now called by the interpreter
- `snapshot()`, `classify_repo()`, `trial_merge()`, `compute_diff()` — observation ops
- All helper functions

### From src/types/git.rs (GS-23)

- `SyncDecision` enum
- `SkipReason` enum
- `BlockReason` enum
- `sync_decision()`, `decide_clean_pair()`, `maybe_merge_to_target()`

### From src/types/action.rs

- `RepoSyncResult` enum — replaced by `OpResult`
- `SyncResult` struct — replaced by `ProgramResult`

Keep:
- `RepoSyncAction` — still used in SessionSyncPlan for observation + preview data
- `SessionSyncPlan` — still the plan type
- `Plan<A>` — still wraps plans

### From src/render.rs

- `sync_plan_inner()` — replaced by `SyncProgram::render_preview()`
- `sync_plan_directed()` — replaced by program preview
- `render_diffstat()` — moved to op preview renderer
- `render_hash_line()` — moved to op preview renderer

Keep:
- `rule()` — generic rendering utility
- `session_info()` — not part of sync
- `sync_result()` — rewrite to render ProgramResult
- `display_name()` — still useful

### From tests/

- Migrate `triple_test.rs` — assert on PullAction/PushAction
- Migrate `integration_test.rs` — use program pipeline
- Migrate `types_test.rs` — use new types
- Migrate `sync_e2e_test.rs` — use program pipeline
- Delete tests that only test SyncDecision

## Acceptance criteria

- `cargo check` — zero warnings about unused sync types
- `cargo test` — all tests pass, no test references SyncDecision
- No `dispatch_pull`/`dispatch_push`/`execute_sync`/`execute_push` in codebase
- Programs are the only way to execute sync operations
