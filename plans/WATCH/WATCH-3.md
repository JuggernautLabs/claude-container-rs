# WATCH-3: Compact Preview for Watch Mode

blocked_by: []
unlocks: [WATCH-4]

## Problem

The current sync preview is multi-line (20+ lines for a session with many repos). Watch mode needs a compact, overwritable display that fits in a few lines while still being informative.

## Scope

### Compact format

```
[hypno] ↔ main  3 synced · 1 pull (synapse +2) · 1 diverged (plexus-core) · 28 deps
```

One line per session. Updates in-place (indicatif or `\r\x1b[K`).

### Expanded format (on keypress or flag)

```
[hypno] watching (3s interval)
  synced:   plexus-macros, plexus-transport, hub-codegen
  pull:     synapse (+2 commits)  plexus-protocol (+1)
  diverged: plexus-core (container +3, host +1)
  dirty:    hypno (2 files)
  deps:     28 unchanged

  last change: 5s ago (container: synapse)
  command: cargo test (exit 0, 2s ago)
```

### Status bar (bottom of terminal)

If running with `-- <cmd>`, the command output fills the terminal. The sync status is a single status bar at the bottom (like tmux status line):

```
[hypno] 3✓ 1← 1↔ | cargo test: ✓ (2s) | 5s ago
```

### Implementation

Use `indicatif` for the compact line (already a dependency). For the status bar with command output, use `crossterm` alternate screen or just stderr for status + stdout for command.

## Files to create

- `src/render.rs` — add `compact_sync_status()` and `status_bar()` functions
