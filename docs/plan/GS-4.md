# GS-4: Safety Gates — Confirmation & Rollback

blocked_by: []
unlocks: [GS-12]

## Problem
1. `session stop` has no confirmation — silently stops a running container
2. `execute_sync()` has no rollback — if repo 3 fails after repos 1-2 succeeded, state is inconsistent
3. Rebuild removes old container before validating new image is buildable
4. Pending merge in `cmd_pull` doesn't prompt per-repo

## Scope
- Add `confirm()` gates to all missing destructive operations
- Add pre-validation before destructive operations (validate image before removing container)
- Add rollback logging (at minimum: print what was done so user can undo)
- Ensure `--yes` / `-y` works consistently everywhere

## TDD Plan

### Tests to write FIRST (in tests/safety_test.rs):

```rust
#[test]
fn session_stop_requires_confirmation() {
    // Parse CLI args for "session -s foo stop" — verify it calls confirm()
    // (test the function signature, not the interactive prompt)
}

#[test]
fn rebuild_validates_image_before_removing_container() {
    // Mock: image build fails
    // Assert: old container still exists after failure
}

#[test]
fn execute_sync_reports_partial_failure() {
    // Given: 3 repos, repo 2 fails
    // Assert: result contains success for repo 1, failure for repo 2, skipped for repo 3
    // Assert: result.partial() returns true
}

#[test]
fn yes_flag_skips_all_confirmations() {
    // Verify confirm("msg", true) returns true without stdin
}

#[test]
fn confirm_defaults_to_yes_on_empty_input() {
    // Enter key (empty string) should confirm
}
```

## Files to modify
- `src/main.rs` — add `confirm()` to `cmd_session_stop`, pending merge loop
- `src/main.rs` — `cmd_session_rebuild`: validate image THEN remove container
- `src/sync/mod.rs` — `execute_sync()`: add `partial` flag to SyncResult
- `src/types/action.rs` — add `SyncResult::partial()` method

## Acceptance criteria
- `session stop` prompts unless `-y`
- `rebuild` only removes container after successful image build
- Partial sync failures are clearly reported with what succeeded
- `--yes` consistently skips all prompts across all commands

## Outcome

**Status:** DONE

**Key code changes:**
- `src/main.rs`: cmd_session_stop takes auto_yes, prompts before stopping
- `src/main.rs`: cmd_session_rebuild builds image BEFORE removing container
- `src/types/action.rs`: Added SyncResult::is_partial() method

**Tests:** 10 in safety_test.rs

**Bugs found:** None
