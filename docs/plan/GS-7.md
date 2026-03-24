# GS-7: Extract Accuracy — Commit Counting & Bundle Cleanup

blocked_by: [GS-3]
unlocks: []

## Problem
1. `extract()` commit count uses `revwalk` from new_head which counts ALL reachable commits, not just new ones. After first extract, count is wrong.
2. Bundle fetch fallback creates `refs/cc-bundle/*` refs that accumulate forever.
3. Bundle creation uses `--all` which bundles the entire repo history — slow for large repos.

## Scope
- Fix commit counting: count from merge-base(old_session_head, new_head) or from old_session_head
- Clean up `refs/cc-bundle/*` after successful fetch
- Use targeted bundle (HEAD only, not --all) with proper ref handling
- Add bundle size logging for large repos

## TDD Plan

### Tests to write FIRST (in tests/extract_test.rs):

```rust
#[tokio::test]
#[ignore]
async fn extract_counts_only_new_commits() {
    // Setup: repo with 10 commits, session branch at commit 5
    // Extract: container at commit 10
    // Assert: commit_count = 5, not 10
}

#[tokio::test]
#[ignore]
async fn extract_first_time_counts_all() {
    // Setup: repo with 3 commits, no session branch
    // Extract: creates session branch
    // Assert: commit_count = 3
}

#[tokio::test]
#[ignore]
async fn extract_cleans_up_bundle_refs() {
    // Setup: extract a repo
    // Assert: no refs/cc-bundle/* remain in host repo
}

#[tokio::test]
#[ignore]
async fn extract_handles_large_repo() {
    // Setup: repo with 1000+ commits
    // Assert: bundle creation completes, commit_count correct
}
```

## Files to modify
- `src/sync/mod.rs` — `extract()`: fix commit counting, add bundle ref cleanup, targeted bundle
- `src/sync/mod.rs` — add `cleanup_bundle_refs()` helper

## Acceptance criteria
- Commit counts match `git rev-list --count session..HEAD`
- No orphaned refs after extract
- Bundle only contains needed refs (not full history)

## Outcome

**Status:** DONE

**Key code changes:**
- `src/sync/mod.rs` extract(): Counts only new commits via old_session_oid delta
- `src/sync/mod.rs`: Added cleanup_bundle_refs() to remove refs/cc-bundle/*
- `src/sync/mod.rs`: Bundle uses HEAD not --all, handles detached HEAD

**Tests:** 7 unit + 6 integration in extract_test.rs

**Bugs found:** None
