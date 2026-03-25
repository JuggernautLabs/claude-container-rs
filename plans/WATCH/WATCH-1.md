# WATCH-1: Watch Mode Epic

## Goal

Dev mode that watches for changes in both the container and host, shows sync previews in real-time, and runs arbitrary commands on change. Like `cargo watch` but for container↔host sync.

## Core Concept

```bash
git-sandbox session -s hypno watch                    # watch and show sync status
git-sandbox session -s hypno watch -- cargo test      # also run command on change
git-sandbox session -s hypno watch --auto-push -- npm run dev  # auto-push host changes in
git-sandbox session -s hypno watch --auto-pull        # auto-extract on container changes
```

The watch loop:
1. Snapshot container state (periodic or inotify)
2. Compare with host
3. Render sync preview (single-line or compact TUI)
4. If `--auto-push` / `--auto-pull`: execute sync automatically
5. If `-- <cmd>`: run the command on each change cycle

## Dependency DAG

```
WATCH-2 (change detection) ──┐
                              ├─→ WATCH-4 (watch loop + TUI)
WATCH-3 (compact preview) ───┤
                              └─→ WATCH-5 (auto-sync modes)
```

## Phases

**Phase 1:** WATCH-2, WATCH-3
**Phase 2:** WATCH-4, WATCH-5
