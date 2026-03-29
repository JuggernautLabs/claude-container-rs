# OPS-3: Cleanup — Inject Failure Leaves Container Dirty

blocked_by: []
unlocks: [OPS-6]

## Bug

When `inject()` runs `git merge` inside a throwaway container and the
merge fails (exit 1), the script exits before `git remote remove
_cc_upstream`. The container is removed but the volume is left with:

- MERGE_HEAD present
- `_cc_upstream` remote still registered
- Conflict markers in working tree
- Dirty status

Next `inject()` call fails because the remote already exists and the
worktree is dirty.

## Fix

The container script (lines 1240-1246 in sync/mod.rs) currently:
```bash
git remote add _cc_upstream /upstream &&
git fetch _cc_upstream {branch} &&
git merge "_cc_upstream/{branch}" --no-edit || exit 1
git remote remove _cc_upstream
```

Change to:
```bash
git remote add _cc_upstream /upstream &&
git fetch _cc_upstream {branch} &&
git merge "_cc_upstream/{branch}" --no-edit
merge_rc=$?
git remote remove _cc_upstream 2>/dev/null
if [ $merge_rc -ne 0 ]; then
    git merge --abort 2>/dev/null
    exit 1
fi
```

This ensures:
1. Remote is always removed (even on merge failure)
2. Merge state is always aborted on failure
3. Volume is left clean regardless of merge outcome

## Test

```rust
#[tokio::test]
#[ignore]  // requires Docker
async fn inject_failure_leaves_volume_clean() {
    // Setup: container repo with file A, host has conflicting file A
    // inject() should fail (merge conflict)
    // Then: snapshot container — verify no MERGE_HEAD, no _cc_upstream,
    //       worktree clean
    // Then: inject() again with non-conflicting change — should succeed
}
```

## Acceptance criteria

- inject() failure leaves volume with clean worktree
- inject() failure removes _cc_upstream remote
- inject() can be retried after a failed inject()
