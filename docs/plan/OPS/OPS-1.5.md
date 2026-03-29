# OPS-1.5: Verify inject works end-to-end

blocked_by: []
unlocks: [OPS-2]

## Problem

During live testing of the two-leg state model, `git-sandbox push`
reported success ("squash-merge 9 commit(s)") but the container HEAD
did not change (stayed at 280e53a). Running push again showed the
same work pending with +1 commit.

Two possible causes:

1. **Dispatch was wrong (fixed).** The old combined_action logic
   picked MergeToTarget (host-side merge) instead of Inject
   (container-side push). Fixed by typed `execute_push()` which
   only dispatches `PushAction`. The result message "squash-merge"
   confirmed it was merging on host, not injecting into container.

2. **Inject itself is broken.** Even with correct dispatch, the
   `inject()` function may be failing silently. The throwaway
   container runs a merge script that could fail without surfacing
   the error properly. OPS-3 fixes cleanup, but the inject may
   have a more fundamental issue.

## Verification steps

### Step 1: Manual test with current binary

```bash
# Pick a session with a repo where target is ahead of container
git-sandbox push -s <session> <branch> --filter '<repo>'

# Check the hash output:
#   container:<hash1>  session:<hash1>  main:<hash2>
# hash1 != hash2 means there's work to push

# Execute the push, then immediately check:
git-sandbox push -s <session> <branch> --filter '<repo>'

# If container hash changed → inject works, old bug was dispatch only
# If container hash unchanged → inject is broken
```

### Step 2: If inject is broken, diagnose

Run inject in isolation with verbose output:

```bash
# Snapshot before
git-sandbox status -s <session> <branch> --filter '<repo>'

# Run inject with RUST_LOG=debug
RUST_LOG=debug git-sandbox push -s <session> <branch> --filter '<repo>'

# Check container logs from the throwaway container
# The inject script is:
#   git remote add _cc_upstream /upstream
#   git fetch _cc_upstream <branch>
#   git merge _cc_upstream/<branch> --no-edit
#   git remote remove _cc_upstream
```

Possible failure points:
- `/upstream` mount not reaching the right host path
- `git fetch` failing (host branch not accessible from container)
- `git merge` failing (conflict, dirty worktree from prior failed inject)
- Container exit code not being checked properly
- Merge succeeds but on wrong branch (not HEAD)

### Step 3: Fix whatever is found

If dispatch-only (already fixed): document, move on to OPS-2.

If inject script: fix the script in sync/mod.rs `inject()` method
(lines ~1240-1250). Common fixes:
- Add error checking per command (`set -e` or `|| exit 1` per step)
- Ensure merge targets HEAD explicitly (`git merge ... HEAD`)
- Clean up prior state before attempting (`git merge --abort 2>/dev/null; git remote remove _cc_upstream 2>/dev/null`)

If mount issue: check that `inject()` mounts the host repo at
`/upstream` and the session volume at `/workspace`.

### Step 4: Confirm with second push

After fix, push should be idempotent:
```bash
git-sandbox push -s <session> <branch> --filter '<repo>'
# → "1 ready, inject N commit(s)"
# Execute
git-sandbox push -s <session> <branch> --filter '<repo>'
# → "Nothing to push" or "0 ready, N unchanged"
```

## Acceptance criteria

- Push with `PushAction::Inject` actually changes container HEAD
- Second push after successful first shows no work remaining
- inject() failure produces a visible error (not silent success)
