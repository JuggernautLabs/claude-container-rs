# GS-5: Dead Flags & Unused Code Cleanup

blocked_by: []
unlocks: []

## Problem
- `--continue` flag on `start` is accepted but ignored (`_continue_session`)
- `--prompt` flag on `start` is accepted but ignored
- `--no-verify` on `pull`/`sync` is accepted but ignored
- `--squash` on `pull` is accepted but squash is always true
- `--strategy` on `push` is accepted but ignored
- `extract: bool` on ProjectConfig is redundant with `role` (dependency = no extract)
- `SessionSyncPlan::execute()` is intentionally unimplemented (uses async instead)

## Scope
- Either implement or remove each dead flag
- Wire `--continue` → `CONTINUE_SESSION=1` env var in container
- Wire `--prompt` → initial prompt passed to Claude
- Wire `--squash` → `merge(squash: bool)` parameter
- Remove `extract` field from ProjectConfig (use role instead)
- Clean up unused imports and variables

## TDD Plan

### Tests to write FIRST (in tests/flags_test.rs):

```rust
#[test]
fn continue_flag_sets_env_var() {
    // Build container args with continue=true
    // Assert: env contains CONTINUE_SESSION=1
}

#[test]
fn squash_false_uses_merge_commit() {
    // Create test repos, merge with squash=false
    // Assert: merge commit has two parents (not squash)
}

#[test]
fn squash_true_uses_single_parent() {
    // Create test repos, merge with squash=true
    // Assert: merge commit has one parent (squash)
}

#[test]
fn role_dependency_implies_no_extract() {
    // ProjectConfig with role=Dependency
    // Assert: filtered out of plan_sync by default
}
```

## Files to modify
- `src/container/mod.rs` — `build_create_args()`: wire continue + prompt
- `src/main.rs` — pass squash flag through to merge; remove unused params
- `src/types/config.rs` — remove `extract` field, use `role` everywhere
- All callers of `extract` field

## Acceptance criteria
- `cargo build` produces zero "unused variable" warnings for flag parameters
- `--continue` resumes previous Claude conversation
- `--squash false` creates merge commits
- No dead code in flag handling

## Outcome

**Status:** DONE

**Key code changes:**
- `src/container/mod.rs`: LaunchOptions struct, --continue sets CONTINUE_SESSION=1, --prompt sets CLAUDE_INITIAL_PROMPT
- `src/types/config.rs`: Removed extract field, role replaces it
- `src/main.rs`: --squash flag threaded through to engine.merge()

**Tests:** 8 in flags_test.rs (1 failing: squash_false_uses_merge_commit needs diverged history fix)

**Bugs found:** None
