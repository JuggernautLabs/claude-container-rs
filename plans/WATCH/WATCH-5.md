# WATCH-5: Auto-Sync Modes

blocked_by: [WATCH-4]
unlocks: []

## Problem

Watch mode shows changes but doesn't act on them by default. Auto-sync modes let the watch loop automatically push/pull without prompting, creating a live sync experience.

## Scope

### Flags

```bash
git-sandbox session -s hypno watch --auto-push              # host changes → push to container
git-sandbox session -s hypno watch --auto-pull              # container changes → extract to host
git-sandbox session -s hypno watch --auto-sync              # both directions
git-sandbox session -s hypno watch --auto-push -- npm start # push + run dev server
```

### Auto-push behavior

When host repo HEAD changes (new commit, branch switch):
1. Show what changed (compact: `← synapse +1 commit`)
2. Run `inject` for the changed repo
3. Update status bar

Use case: editing on host, container needs the changes for its build.

### Auto-pull behavior

When container repo HEAD changes (Claude committed):
1. Show what changed (compact: `→ synapse +1 commit`)
2. Run `extract` for the changed repo (to session branch)
3. Optionally run `-- <cmd>` to test the changes

Use case: Claude is working, you want to see changes in real-time.

### Auto-sync behavior

Both directions. Conflict detection prevents loops:
- If container changed since last push → pull first
- If host changed since last pull → push first
- If both changed → show diverged, don't auto-sync (would conflict)

### Debouncing

Don't sync on every keystroke. Wait for a quiet period:
- Host file saves: 500ms debounce
- Git commits (HEAD change): sync immediately
- Container polls: already on 3s interval

### Status display with auto-sync

```
[hypno] auto-sync ↔  3✓ · ←synapse +1 (2s ago) · →plexus +1 (5s ago) | npm start: running
```

## Files to modify

- `src/watch.rs` — auto-sync logic, debouncing, conflict prevention
