# VM-1.5: Define Backend Trait Skeleton + Rewrite Tests Against It

blocked_by: [OPS-2, OPS-3, OPS-4, OPS-5]
unlocks: [VM-2]

## Problem

OPS-2 tests call `SyncEngine::merge()` directly and assert on
`MergeOutcome` variants. When VM-2 decomposes merge into primitives,
these tests break — not because behavior changed, but because the
API changed. The assertions are right, the entry point is wrong.

## Fix

1. Define the `Backend` trait now (just the signature, no VM yet)
2. Implement it on `SyncEngine` (thin wrapper)
3. Rewrite test entry points to call through the trait
4. Rewrite assertions to check git state, not return types

The trait becomes the contract that VM-2 implements. Tests written
against it carry over unchanged.

## The trait (skeleton)

```rust
pub trait Backend: Send + Sync {
    // Compound ops (what tests call today)
    fn merge(&self, repo_path: &Path, from: &str, to: &str, squash: bool)
        -> Result<(), BackendError>;
    async fn extract(&self, session: &SessionName, repo: &str, host: &Path, branch: &str)
        -> Result<(), BackendError>;
    async fn inject(&self, session: &SessionName, repo: &str, host: &Path, branch: &str)
        -> Result<(), BackendError>;

    // Observation
    fn ref_head(&self, repo_path: &Path, branch: &str) -> Option<String>;
    fn is_worktree_clean(&self, repo_path: &Path) -> bool;
    fn has_conflict_markers(&self, repo_path: &Path, branch: &str) -> bool;
}
```

Note: this starts at the compound level (merge, extract, inject) —
not the 12 primitives. VM-2 will decompose these into primitives.
The trait evolves, but tests stay stable because they assert on
git state, not on trait method return types.

## Test rewrite

Before:
```rust
let engine = SyncEngine::new(docker);
let result = engine.merge(&path, "session", "main", true);
assert!(matches!(result, MergeOutcome::SquashMerge { commits: 3, .. }));
```

After:
```rust
let backend = SyncEngine::new(docker);  // implements Backend
let head_before = backend.ref_head(&path, "main").unwrap();

backend.merge(&path, "session", "main", true).unwrap();

let head_after = backend.ref_head(&path, "main").unwrap();
assert_ne!(head_before, head_after, "target should advance");
assert_eq!(commit_count_since(&path, "main", &head_before), 1, "one squash commit");
assert!(!backend.has_conflict_markers(&path, "main"));
assert!(backend.is_worktree_clean(&path));
```

The assertion checks what the user would see: target advanced by
one commit, no markers, worktree clean. Doesn't mention MergeOutcome.

## What changes in each test

| Test | MergeOutcome assertion | Git state assertion |
|---|---|---|
| merge_ff_advances_target | FastForward{2} | target HEAD == session HEAD |
| merge_squash_creates_single_commit | SquashMerge{3} | 1 new commit, tree matches session |
| merge_squash_only_new_commits | SquashMerge{2} | 2 commits counted since squash-base |
| merge_conflict_preserves_target | Conflict{..} | target HEAD unchanged |
| merge_conflict_no_markers | Conflict{..} | no markers on target |
| merge_conflict_worktree_clean | Conflict{..} | worktree clean |
| merge_noop_when_up_to_date | AlreadyUpToDate | target HEAD unchanged |
| merge_noop_when_behind | AlreadyUpToDate | target HEAD unchanged |
| merge_blocked_when_host_dirty | Err | target HEAD unchanged |
| merge_blocked_when_no_session | Err | target HEAD unchanged |

The conflict tests still need to know a conflict HAPPENED (to verify
rollback). The Backend trait returns `Result<(), BackendError>` where
`BackendError::Conflict { files }` carries the info. But the TARGET
assertions are all git state.

## Scope

- New file: `src/backend.rs` (trait definition)
- `impl Backend for SyncEngine` (thin wrappers)
- Rewrite `tests/two_leg_test.rs` merge tests to use `Backend` trait
- Pure derivation tests (Part A) unchanged — they test RepoState, not Backend

## What this does NOT include

- VM state (RepoVM, SyncVM) — that's VM-2
- Mock backend — that's VM-3
- 12 primitive decomposition — that's VM-2
- Program generation — that's VM-4/5

This is ONLY: define the interface tests will use, implement it on
what exists, rewrite tests to use it. The trait is the bridge.

## Acceptance criteria

- Backend trait defined with merge/extract/inject + observation methods
- SyncEngine implements Backend
- All merge safety tests assert on git state, not MergeOutcome
- All merge safety tests call through Backend trait
- Every merge test asserts worktree_clean + no conflict markers
- 29 tests still pass
- No test imports MergeOutcome (decoupled from internal types)
