# GS-8: Container & Ref Orphan Cleanup

blocked_by: [GS-4]
unlocks: []

## Problem
1. Throwaway containers (cc-validate-*, cc-snap-*, cc-extract-*, cc-clone-*, cc-inject-*) accumulate if cleanup fails
2. No periodic garbage collection
3. `session cleanup` only removes marker files, not orphaned containers
4. `ls` shows stale sessions (metadata but no volumes)

## Scope
- Add `session gc` subcommand that removes orphaned containers + stale metadata
- Label all throwaway containers so they can be found
- `session cleanup` also removes orphaned throwaway containers for the session
- `ls` optionally shows only active sessions (`ls --active`)

## TDD Plan

### Tests to write FIRST (in tests/cleanup_test.rs):

```rust
#[tokio::test]
#[ignore]
async fn gc_removes_orphaned_throwaway_containers() {
    // Create a container with label claude-container.throwaway=true
    // Run gc
    // Assert: container removed
}

#[tokio::test]
#[ignore]
async fn gc_preserves_session_containers() {
    // Create a session container (claude-session-ctr-*)
    // Run gc
    // Assert: container preserved
}

#[test]
fn ls_active_filters_stale_sessions() {
    // Given: metadata for session-a (has volumes), session-b (no volumes)
    // ls --active shows only session-a
}

#[tokio::test]
#[ignore]
async fn cleanup_removes_session_throwaway_containers() {
    // Create cc-snap-foo-* container
    // Run session cleanup -s foo
    // Assert: container removed
}
```

## Files to modify
- `src/main.rs` — add `session gc` subcommand, `ls --active` flag
- `src/sync/mod.rs` — add `claude-container.throwaway=true` label to all throwaway containers
- `src/lifecycle/mod.rs` — add label to validation containers
- `src/main.rs` — `cmd_session_cleanup` also cleans containers

## Acceptance criteria
- `session gc` removes all orphaned throwaway containers
- `ls --active` shows only sessions with volumes
- All throwaway containers are labeled for discovery

## Outcome

**Status:** DONE

**Key code changes:**
- All throwaway containers labeled claude-container.throwaway=true
- `src/main.rs`: gc command removes orphaned throwaway containers
- `src/main.rs`: ls --active filters metadata-only ghosts
- Session cleanup also removes session-scoped throwaway containers

**Tests:** 3 unit + 7 integration in cleanup_test.rs

**Bugs found:** None
