# WATCH-2: Change Detection

blocked_by: []
unlocks: [WATCH-4]

## Problem

Currently sync state is computed on-demand (run `pull --dry-run`). Watch mode needs to detect changes efficiently without running a full snapshot every second.

## Scope

### Host-side detection

Host repos are local — use filesystem watching:

```rust
// Watch .git/HEAD, .git/refs/heads/*, and working tree
// Libraries: notify (Rust crate) or polling fallback
```

Triggers: new commit, branch switch, file save, stage/unstage.

### Container-side detection

Container repos are in Docker volumes — can't use inotify from host.

Options (in order of preference):
1. **Polling via docker exec**: run `git rev-parse HEAD` periodically in the running container. Lightweight — one exec per repo per cycle.
2. **Volume mount + inotify**: mount the session volume on host at a temp path, watch `.git/HEAD`. Only works if volume driver supports it (not on Colima).
3. **Container-side watcher**: run a sidecar process in the container that watches and signals. Overkill.

Recommendation: option 1 (polling). Configurable interval (default 3s).

```rust
struct ChangeDetector {
    session: SessionName,
    poll_interval: Duration,
    last_host_heads: HashMap<String, String>,     // repo → HEAD sha
    last_container_heads: HashMap<String, String>,
}

impl ChangeDetector {
    async fn poll(&mut self) -> Vec<ChangeEvent> {
        // Check host HEADs via git2 (fast, local)
        // Check container HEADs via docker exec (one call, all repos)
        // Return list of repos that changed since last poll
    }
}

enum ChangeEvent {
    HostChanged { repo: String, old_head: String, new_head: String },
    ContainerChanged { repo: String, old_head: String, new_head: String },
    HostDirtyChanged { repo: String, dirty_count: u32 },
}
```

### Efficiency

- Host checks: git2 `repo.head()` is instant — run every cycle
- Container checks: `docker exec` with a single script checking all repos — one exec per cycle, not per repo
- Full snapshot (diff computation) only when a change is detected

## Files to create

- `src/watch.rs` — ChangeDetector, ChangeEvent, poll loop
