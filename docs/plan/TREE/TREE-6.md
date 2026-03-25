# TREE-6: Fork — Volume Snapshot + New Container

blocked_by: [TREE-4, TREE-5]
unlocks: []

## Problem

No way to branch off an experiment without losing your current session state. "Fork" = snapshot the session volume into a new volume, create a new Fork container pointing at it.

## Scope

### `session fork` subcommand

```bash
git-sandbox session -s hypno fork experiment "testing GPU approach"
git-sandbox session -s hypno fork gpu --dockerfile ./Dockerfile.gpu
```

### What fork does

1. **Snapshot session volume**: throwaway container copies `claude-session-{name}` → `claude-session-{name}--{label}`
2. **Fresh state volume**: create `claude-state-{name}--{label}` (empty — new conversation)
3. **Shared cache volumes**: cargo/npm/pip remain shared (same volume names)
4. **Register fork**: store in session metadata as `ContainerRole::Fork(label)`
5. **Start fork container**: `claude-fork-{label}-ctr-{session}` mounting the forked volumes

### Volume naming for forks

```rust
impl SessionName {
    pub fn fork_session_volume(&self, label: &str) -> VolumeName {
        VolumeName(format!("claude-session-{}--{}", self.0, label))
    }
    pub fn fork_state_volume(&self, label: &str) -> VolumeName {
        VolumeName(format!("claude-state-{}--{}", self.0, label))
    }
    // cargo/npm/pip shared — no fork variants needed
}
```

### Volume snapshot via throwaway container

```rust
pub async fn snapshot_volume(
    &self,
    source: &VolumeName,
    target: &VolumeName,
    session: &SessionName,
) -> Result<(), ContainerError> {
    // 1. Create target volume
    // 2. Throwaway container:
    //    mount source at /src:ro, target at /dst
    //    cp -a /src/. /dst/
    //    RunAs::developer()
}
```

### fork_session_volume() in build_create_args

When role is Fork(label), the container mounts:
- `claude-session-{name}--{label}` at `/workspace` (forked)
- `claude-state-{name}--{label}` at `~/.claude` (fresh)
- Shared: cargo/npm/pip (same as parent)

### Session show with forks

```
session: hypno
  containers:
    ● work         running   claude-session-ctr-hypno
    · experiment   no ctr    (forked volumes: claude-session-hypno--experiment)
```

## TDD Plan

```rust
#[tokio::test]
#[ignore]
async fn fork_creates_new_volumes() {
    // fork "test-fork" from session "test-parent"
    // assert: claude-session-test-parent--test-fork volume exists
    // assert: claude-state-test-parent--test-fork volume exists
}

#[tokio::test]
#[ignore]
async fn fork_copies_workspace() {
    // write file to parent session volume
    // fork
    // read file from forked volume → same content
}

#[tokio::test]
#[ignore]
async fn fork_does_not_copy_state() {
    // parent has .claude.json in state volume
    // fork
    // forked state volume is empty (fresh conversation)
}
```

## Files to modify

- `src/types/ids.rs` — `fork_session_volume()`, `fork_state_volume()`
- `src/lifecycle/mod.rs` — `snapshot_volume()`
- `src/container/mod.rs` — `build_create_args` checks role for fork volumes
- `src/main.rs` — `session fork` subcommand, creates volumes + starts container
- `src/render.rs` — show fork info in session display

## Acceptance criteria

- `session fork experiment` creates new volumes with content from parent
- Forked container has its own workspace (changes don't affect parent)
- Forked container has fresh conversation state
- Cargo/npm/pip caches shared (faster startup)
- `session show` displays forks with their volume names
