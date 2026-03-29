# TREE-5: `-c` Flag, Session Show, Per-Container Operations

blocked_by: [TREE-3]
unlocks: []

## Problem

All commands assume the default (Work) container. No way to start, stop, attach, or rebuild a specific container by role.

## Scope

### `-c` flag on Start

```rust
Start {
    #[arg(short = 'c', long = "container")]
    container: Option<String>,  // parsed into ContainerRole at boundary
    ...
}
```

Parsing:
- None / "work" → `ContainerRole::Work`
- "reconcile" → `ContainerRole::Reconcile`
- "run" → `ContainerRole::Runner`
- anything else → `ContainerRole::Fork(value)`

### Parent tracking when spawning

When starting a non-Work container, the parent is the currently active container:

```rust
let parent = discovered.active_container().map(|c| c.name.clone());
// Store parent as a Docker label on the new container:
//   claude-container.parent=claude-session-ctr-hypno
// Recovered during discovery via inspect → labels
```

The parent is stored as a Docker label (not in metadata files) so it survives across process invocations and is always in sync with the actual container.

### `-c` flag on session stop/rebuild/exec

```rust
SessionAction::Stop {
    #[arg(short = 'c', long = "container")]
    container: Option<String>,
}
```

`None` = stop all. `Some("reconcile")` = stop just reconcile.

### session show — container tree

```
session: hypno
  dockerfile: /path/to/Dockerfile
  rootish: true

  containers:
    ● work         running   claude-session-ctr-hypno
    ○ reconcile    stopped   claude-reconcile-ctr-hypno

  projects: (14)
    ...
```

### start -a attaches to active container

```rust
// Find the active container (running one, reconcile takes priority)
if let Some(active) = discovered.active_container() {
    eprintln!("  Attaching to {} ({})...", active.name, active.role);
    attach_to_running(&lc, &active.name, replay_logs).await?;
}
```

### Detach message includes container info

```
→ Detached from claude-reconcile-ctr-hypno (reconcile)
  To reattach: git-sandbox start -s hypno -a
  Other containers: work (stopped)
```

## TDD Plan

```rust
#[test]
fn parse_container_flag_work() {
    assert_eq!(parse_role(None), ContainerRole::Work);
    assert_eq!(parse_role(Some("work")), ContainerRole::Work);
}

#[test]
fn parse_container_flag_reconcile() {
    assert_eq!(parse_role(Some("reconcile")), ContainerRole::Reconcile);
}

#[test]
fn parse_container_flag_fork() {
    assert_eq!(parse_role(Some("my-experiment")), ContainerRole::Fork("my-experiment".into()));
}

#[test]
fn active_container_prefers_reconcile() {
    // Given: work=stopped, reconcile=running
    // active_container() returns reconcile
}
```

## Files to modify

- `src/main.rs` — add `-c` to Start/Stop/Rebuild/Exec, parse into ContainerRole
- `src/main.rs` — `cmd_start` uses role for container_name_for()
- `src/main.rs` — `cmd_session_stop` accepts optional role
- `src/container/mod.rs` — detach message includes container name + role
- `src/render.rs` — `session_info` shows container tree

## Acceptance criteria

- `start -s hypno` → Work container (unchanged)
- `start -s hypno -c reconcile` → Reconcile container
- `session -s hypno stop -c work` → stops only work
- `start -s hypno -a` → attaches to whichever is running
- Detach says which container you were in
