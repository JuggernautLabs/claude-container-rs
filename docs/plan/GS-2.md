# GS-2: Typed Error Variants for Sync Operations

blocked_by: []
unlocks: [GS-6, GS-12]

## Problem
Conflict detection in `collect_conflicts()` (main.rs:1561) uses `.contains("Conflict")` string matching on error messages. If the message format changes, conflict detection silently breaks and agentic reconciliation never triggers.

Also: `ExtractionFailed` is a catch-all for bundle errors, fetch errors, merge errors. Can't distinguish programmatically.

## Scope
- Add typed error variants for merge conflicts, extraction failures, injection failures
- Replace string matching with pattern matching on error types
- Propagate conflict file lists through error types

## TDD Plan

### Tests to write FIRST (in tests/error_types_test.rs):

```rust
#[test]
fn merge_conflict_error_carries_file_list() {
    // MergeConflict { repo, files } variant exists and carries data
}

#[test]
fn collect_conflicts_uses_typed_errors_not_strings() {
    // Given a SyncResult with MergeConflict errors,
    // collect_conflicts returns them without string matching
}

#[test]
fn extraction_failed_distinguishes_bundle_vs_fetch() {
    // BundleFailed vs FetchFailed vs BranchCreateFailed
}

#[test]
fn inject_failed_is_distinct_from_extraction() {
    // InjectionFailed { repo, reason } is its own variant
}
```

## Files to modify
- `src/types/error.rs` — add `MergeConflict { repo, files }`, split `ExtractionFailed` into subtypes
- `src/sync/mod.rs` — return typed errors from merge(), extract(), inject()
- `src/main.rs` — replace `collect_conflicts()` string matching with type matching

## Acceptance criteria
- No `.contains("Conflict")` or `.contains("conflict")` anywhere in main.rs
- All sync error types are exhaustively matchable
- Conflict file lists propagate through the error chain

## Outcome

**Status:** DONE

**Key code changes:**
- `src/types/error.rs`: Added MergeConflict, BundleFailed, FetchFailed, BranchCreateFailed, InjectionFailed error variants
- `src/types/action.rs`: Added RepoSyncResult::Conflicted { repo_name, files } variant
- `src/sync/mod.rs`: extract/merge/inject return typed errors instead of string-based errors
- `src/main.rs`: collect_conflicts() uses pattern matching, zero string matching

**Tests:** 5 in error_types_test.rs

**Bugs found:** None
