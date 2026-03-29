# OPS-1.5: Verify inject works end-to-end + idempotency

blocked_by: []
unlocks: [OPS-2]

## Status: COMPLETE

All acceptance criteria met. Changes shipped in v0.3.0.

## What was fixed

### 1. Dispatch bug (the original squash-push issue)
`execute_sync` had a combined_action that preferred pull over push.
`MergeToTarget` won over `Inject`, so push did a host-side merge
instead of a container-side inject. Fixed by typed `execute_push()`
which only dispatches `PushAction` (4 variants, can't merge).

### 2. Inject idempotency
After inject, session branch wasn't re-extracted. Next push still
showed work because session was stale. Fixed: `dispatch_push` now
re-extracts after successful inject to sync the session branch.

### 3. Host dirty doesn't block push
Push reads a committed ref, not the worktree. `push_action()` no
longer returns `Blocked` for `HostDirty` or `HostNotARepo` — only
container-side blockers (dirty, merging, rebasing) block push.

### 4. Force push
`--force` flag added. Blocked repos (dirty container, merge in
progress) get `git fetch + git reset --hard + git clean -fd` instead
of being skipped. Force targets show diff preview (what will change).

### 5. Default branch
Push defaults to session name as branch (not main). Override with
positional arg: `git-sandbox push -s http-gateway main`.

### 6. Blocked repo diffs
Blocked repos now show commit hashes and diff summary in the plan
view, so you can see what force-reset would change before confirming.

## Other fixes made during this work

- Unicode truncation panic in build log UI (byte slicing → char slicing)
- Terminal width-adaptive build log (reads `crossterm::terminal::size()`)
- `curl install.sh | sh` → `| bash` (Claude installer has bashisms)
- Claude install verification (fail loudly, not silently continue)
- Bash existence check before entrypoint (clear error on missing bash)
- Terminal restore on container error (keys no longer eaten)
- Log replay on container resume (entrypoint output visible)
- Podman compatibility (`X-Registry-Config` header fix)

## Acceptance criteria — all met

- Push with `PushAction::Inject` actually changes container HEAD ✓
- After successful inject, session branch updated via re-extract ✓
- Second push after successful first shows no work remaining ✓
- inject() failure produces a visible error (not silent success) ✓
- Re-extract failure after inject does not fail the push ✓
- Host dirty does not block push ✓
- `--force` resets blocked repos to match host branch ✓
