# GS-3: Git State Triple — (Container, Session, Target)

blocked_by: []
unlocks: [GS-7, GS-12]

## Problem
`classify_repo()` only compares container HEAD vs session branch HEAD. When they match (already extracted), `pull -s foo main` says "unchanged" even though the session branch has unmerged work ahead of main. The fix in the render layer (session_ahead_of_target) is a workaround — the classify itself should model all three refs.

Also: `read_host_side()` only reads the session branch. It should return both session and target branch state.

## Scope
- Replace `RepoPair { container, host }` with a triple that includes target branch state
- `classify_repo()` computes relations between all three: container↔session, session↔target
- `sync_decision()` uses all three relations
- Remove `session_ahead_of_target` workaround from RepoSyncAction

## TDD Plan

### Tests to write FIRST (in tests/triple_test.rs):

```rust
#[test]
fn triple_container_matches_session_but_session_ahead_of_target() {
    // container=A, session=A, target=B (B is ancestor of A)
    // Decision: MergeToTarget (session→target merge needed)
}

#[test]
fn triple_container_ahead_session_behind_target() {
    // container=C, session=B, target=A (A ancestor of B ancestor of C)
    // Decision: Extract then MergeToTarget
}

#[test]
fn triple_all_same() {
    // container=A, session=A, target=A
    // Decision: Skip (truly nothing to do)
}

#[test]
fn triple_no_session_branch() {
    // container=A, session=None, target=B
    // Decision: Extract (create session branch)
}

#[test]
fn triple_session_diverged_from_target() {
    // container=A, session=A, target=C (diverged from session)
    // Decision: Reconcile
}

#[test]
fn triple_container_ahead_and_session_ahead_of_target() {
    // container=D, session=C, target=A
    // Decision: Extract (C→D), then MergeToTarget (D→target)
}
```

## Files to modify
- `src/types/git.rs` — add `RepoTriple { container, session, target }` and `TripleRelation`
- `src/sync/mod.rs` — `classify_repo()` returns triple; `plan_sync()` uses it
- `src/types/action.rs` — remove `session_ahead_of_target` workaround
- `src/render.rs` — render from triple relations
- `src/main.rs` — remove pending_merge special-case in cmd_pull

## Acceptance criteria
- `pull -s foo main` correctly shows "pending merge" when session branch ahead of main
- `pull -s foo main` correctly shows "extract + merge" when container has new work
- No separate `session_ahead_of_target` field — all state is in the triple
- All 6 triple test cases pass

## Outcome

**Status:** DONE

**Key code changes:**
- `src/types/git.rs`: Added SessionTargetRelation struct, MergeToTarget variant, maybe_merge_to_target() method
- `src/sync/mod.rs`: classify_repo() computes full triple, added read_target_state()
- `src/types/action.rs`: Removed session_ahead_of_target workaround from RepoSyncAction

**Tests:** 6 in triple_test.rs

**Bugs found:** Squash-merge creates content-identical trees with different SHAs. Fixed by checking ContentComparison::Identical in maybe_merge_to_target().
