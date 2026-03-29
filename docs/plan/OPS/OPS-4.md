# OPS-4: Cleanup — Merge Crash Leaves Host Dirty

blocked_by: []
unlocks: [VM-2]

## Status: COMPLETE (uncommitted, pending commit after current HEAD 3db9d43)

MergeGuard struct added with Drop impl. Armed before set_head,
disarmed after successful commit+ref update. Both squash and regular
merge paths guarded.

## Bug

`merge()` has 6 crash/error points between entering merge state and
completing cleanup. If the process dies or git2 returns an error at
any of these points, the host repo is left with merge state (MERGE_HEAD)
and possibly conflict markers in the working tree.

The existing conflict path (lines 924-932, 1002-1006) does:
```rust
repo.cleanup_state()?;
repo.checkout_head(Some(CheckoutBuilder::new().force()))?;
```

But if `cleanup_state()` itself fails, the `?` propagates and
checkout_head never runs. The repo stays dirty.

Crash points:
1. After `set_head()` but before `repo.merge()` — HEAD moved, no merge started
2. After `repo.merge()` but before conflict check — index has merge state
3. After `cleanup_state()` fails — MERGE_HEAD gone but worktree dirty
4. After `commit()` but before `reference()` update — commit exists, ref stale
5. After target ref updated but before squash-base ref — squash tracking corrupted
6. After squash-base but before final `cleanup_state()` — merge state persists

## Fix

Wrap the entire merge operation in a guard that ensures cleanup on any
exit path:

```rust
fn merge(&self, host_path, session_branch, target_branch, squash) -> Result<MergeOutcome> {
    let repo = Repository::open(host_path)?;
    // ... find branches, check up-to-date ...

    // Enter merge state — everything after this needs cleanup on failure
    let _guard = MergeGuard::new(&repo, &target_ref_name);

    // ... set_head, checkout, merge, check conflicts ...
    // ... commit, update refs ...

    _guard.disarm();  // success — don't cleanup
    Ok(outcome)
}

struct MergeGuard<'a> {
    repo: &'a Repository,
    target_ref: String,
    armed: bool,
}

impl<'a> MergeGuard<'a> {
    fn new(repo: &'a Repository, target_ref: &str) -> Self {
        Self { repo, target_ref: target_ref.to_string(), armed: true }
    }
    fn disarm(&mut self) { self.armed = false; }
}

impl Drop for MergeGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = self.repo.cleanup_state();
            let _ = self.repo.checkout_head(Some(
                git2::build::CheckoutBuilder::new().force()
            ));
        }
    }
}
```

The guard ensures: no matter where merge() exits (error, panic, early
return), the host repo's merge state is cleaned up and worktree is
restored to match the target branch ref.

## Test

```rust
#[test]
fn merge_error_leaves_host_clean() {
    // Setup: repo where merge will fail with git error (not conflict)
    // e.g., corrupt object, or mock the commit step to fail
    // After: verify no MERGE_HEAD, worktree matches target HEAD
}

#[test]
fn merge_guard_cleanup_on_drop() {
    // Setup: enter merge state manually, drop guard without disarming
    // After: verify cleanup happened
}
```

## Acceptance criteria

- MergeGuard Drop cleans up merge state on any error path
- No merge() exit leaves MERGE_HEAD in the repo
- No merge() exit leaves conflict markers in worktree
- Existing merge tests still pass (guard is transparent on success)
