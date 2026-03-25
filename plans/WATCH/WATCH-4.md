# WATCH-4: Watch Loop + Command Execution

blocked_by: [WATCH-2, WATCH-3]
unlocks: [WATCH-5]

## Problem

Need a main loop that polls for changes, renders status, and optionally runs a command — all without blocking each other.

## Scope

### CLI

```bash
git-sandbox session -s hypno watch                         # just show status
git-sandbox session -s hypno watch -- cargo test           # run on change
git-sandbox session -s hypno watch --interval 5 -- make    # custom poll interval
git-sandbox session -s hypno watch --filter "synapse" -- cargo test -p synapse
```

### Architecture

Three concurrent tasks:

```
┌─────────────┐     ┌──────────────┐     ┌──────────────┐
│ Poll Loop   │────→│ Change Queue │────→│ Renderer     │
│ (3s cycle)  │     │              │     │ (status bar) │
└─────────────┘     │              │     └──────────────┘
                    │              │
                    │              │────→│ Cmd Runner   │
                    │              │     │ (if -- cmd)  │
                    └──────────────┘     └──────────────┘
```

1. **Poll loop**: runs `ChangeDetector::poll()` every N seconds. Emits `ChangeEvent`s.
2. **Renderer**: updates the compact status line. On change, shows what moved.
3. **Command runner**: spawns the user command. Restarts on each change event. Passes through stdout/stderr.

### Command behavior

```bash
git-sandbox session -s hypno watch -- cargo test
```

- On first run: spawn `cargo test`, show output
- On change detected: show what changed, restart `cargo test`
- `Ctrl-C`: kill the command and exit watch mode
- Command exit: show exit code in status bar, wait for next change

The command runs on the **host**, not in the container. It's for local dev loops:
- Container changes → pull triggers → `cargo test` runs on host to verify
- Host file save → push triggers → command runs to check

### Keyboard shortcuts (while watching)

```
q       — quit
p       — trigger pull now
P       — trigger push now
s       — show full sync preview
Enter   — re-run command
```

### Filter

`--filter` applies to which repos are watched. If only watching `synapse`, changes to `plexus-core` in the container don't trigger.

## Files to create

- `src/watch.rs` — WatchLoop, command spawning, keyboard handler
- `src/main.rs` — `Watch` variant in SessionAction, `cmd_watch()`
