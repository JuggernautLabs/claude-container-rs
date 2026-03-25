# TREE-4: Volume Exclusivity Enforcement

blocked_by: [TREE-2, TREE-3]
unlocks: [TREE-6]

## Problem

Two containers writing to the same session volume simultaneously corrupts git repos and conversation history. Currently reconciliation manually stops the work container first. This needs to be a system-wide invariant.

## Scope

### Volume access rules

| Volume | Concurrent access | Why |
|--------|------------------|-----|
| session (`/workspace`) | Exclusive write | Git repos |
| state (`.claude`) | Exclusive write | Conversation history |
| cargo/npm/pip | Shared | Append-mostly caches |

### Pre-start check (`src/lifecycle/mod.rs`)

```rust
pub async fn check_volume_exclusivity(
    &self,
    session: &SessionName,
    role: &ContainerRole,
) -> Result<Option<SessionContainer>, ContainerError> {
    // List running containers for this session
    let running = self.list_session_containers(session).await?
        .into_iter()
        .filter(|c| c.state.is_running() && c.role != *role)
        .collect::<Vec<_>>();

    if running.is_empty() {
        Ok(None)
    } else {
        Ok(Some(running[0].clone()))  // the blocking container
    }
}
```

### Enforcement in cmd_start (`src/main.rs`)

```rust
if let Some(blocker) = lc.check_volume_exclusivity(name, &role).await? {
    if force {
        eprintln!("  Stopping {} ({})...", blocker.name, blocker.role);
        lc.stop_container(&blocker.name).await?;
    } else {
        eprintln!("  {} {} ({}) is running on the same volumes.",
            "⚠", blocker.name, blocker.role);
        eprintln!("  Use --force to stop it, or stop manually:");
        eprintln!("    git-sandbox session -s {} stop -c {}", name, blocker.role);
        return Ok(());
    }
}
```

### --force flag on Start

```rust
Start {
    ...
    #[arg(long)]
    force: bool,
}
```

## TDD Plan

```rust
#[test]
fn exclusivity_blocks_when_another_running() {
    // Given: work container running
    // Starting reconcile without force → returns blocker
}

#[test]
fn exclusivity_allows_when_none_running() {
    // Given: no containers running
    // Starting work → Ok(None)
}

#[test]
fn force_stops_blocking_container() {
    // Given: work running, --force
    // Starting reconcile stops work first
}
```

## Files to modify

- `src/lifecycle/mod.rs` — add `check_volume_exclusivity()`
- `src/main.rs` — pre-start exclusivity check, `--force` flag
- `src/container/mod.rs` — `launch_reconciliation` uses exclusivity check instead of manual stop

## Acceptance criteria

- Starting a second container when one is running → clear error message
- `--force` stops the blocker and proceeds
- Reconciliation uses the same check (not ad-hoc stop)
