# GS-6: Typed Conflict Detection → Agentic Reconciliation

blocked_by: [GS-2]
unlocks: []

## Problem
The pipeline from merge conflict → agentic reconciliation has gaps:
1. `merge()` returns `MergeOutcome::Conflict` but `execute_sync` wraps it in a string error
2. `collect_conflicts()` parses that string back — fragile
3. `offer_reconciliation()` verification pipeline is duplicated from `cmd_start`
4. Post-reconciliation re-extract doesn't verify the conflict was actually resolved

## Scope
- Use typed `MergeConflict` error from GS-2 end-to-end
- `execute_sync` returns conflicts as structured data, not error strings
- Deduplicate verification pipeline (share with cmd_start)
- Add post-reconciliation verification: check .reconcile-complete AND re-extract succeeds

## TDD Plan

### Tests to write FIRST:

```rust
#[test]
fn execute_sync_returns_conflict_with_files() {
    // Given: diverged repos with known conflicts
    // When: execute_sync runs
    // Then: result contains RepoSyncResult::Conflict { files }
}

#[test]
fn reconciliation_detects_resolved_conflicts() {
    // Given: .reconcile-complete exists in volume
    // When: check_reconcile_complete runs
    // Then: returns true with description
}

#[test]
fn reconciliation_detects_unresolved() {
    // Given: no .reconcile-complete
    // When: check_reconcile_complete runs
    // Then: returns false
}
```

## Files to modify
- `src/types/action.rs` — add `RepoSyncResult::Conflict { repo_name, files }`
- `src/sync/mod.rs` — `execute_sync` returns Conflict variant instead of Failed+string
- `src/main.rs` — `collect_conflicts` pattern matches on typed variant
- `src/main.rs` — extract shared verification into helper
- `src/container/mod.rs` — `check_reconcile_complete` returns description

## Acceptance criteria
- Zero string matching for conflict detection
- Agentic reconciliation triggers reliably from typed errors
- Post-reconciliation verifies work was completed
