# TREE-3: Multi-Container Discovery

blocked_by: [TREE-2]
unlocks: [TREE-4, TREE-5]

## Problem

`SessionManager::discover()` inspects exactly one container by name: `name.container_name()`. It can't see reconcile, fork, or runner containers. `DiscoveredSession` has separate `Stopped`/`Running` variants that hold a single `ContainerInspect`.

## Scope

### Collapse DiscoveredSession (`src/types/session.rs`)

Replace:
```rust
DoesNotExist(SessionName),
VolumesOnly { name, metadata, volumes },
Stopped { name, metadata, volumes, container },   // one container
Running { name, metadata, volumes, container },    // one container
```

With:
```rust
DoesNotExist(SessionName),
VolumesOnly { name, metadata, volumes },
Active { name, metadata, volumes, containers: Vec<SessionContainer> },
```

`Active` replaces both `Stopped` and `Running`. The per-container state is in `SessionContainer.state`.

### list_session_containers() (`src/lifecycle/mod.rs`)

```rust
pub async fn list_session_containers(&self, session: &SessionName) -> Result<Vec<SessionContainer>> {
    // Docker API: list containers with name filter (prefix match)
    // docker.list_containers(filter: name=claude-*-ctr-{session})
    //
    // For each container:
    //   1. Parse name → (SessionName, ContainerRole) via parse_container_name
    //   2. Inspect → ContainerInspect
    //   3. Build SessionContainer { role, name, state, info }
    //
    // Also check for the legacy name pattern (claude-session-ctr-{session})
}
```

### Update discover() (`src/session/mod.rs`)

```rust
pub async fn discover(&self, name: &SessionName) -> Result<DiscoveredSession> {
    let volumes = self.inspect_volumes(name).await?;
    let metadata = self.load_metadata(name);

    if !volumes.session.exists() && !volumes.state.exists() {
        return Ok(DiscoveredSession::DoesNotExist(name.clone()));
    }

    let containers = lc.list_session_containers(name).await?;

    if containers.is_empty() {
        Ok(DiscoveredSession::VolumesOnly { name, metadata, volumes })
    } else {
        Ok(DiscoveredSession::Active { name, metadata, volumes, containers })
    }
}
```

### Helper methods on Active

```rust
impl DiscoveredSession {
    /// The "active" container — whichever is running. Work takes priority unless reconcile is running.
    pub fn active_container(&self) -> Option<&SessionContainer> { ... }

    /// Find container by role
    pub fn container(&self, role: &ContainerRole) -> Option<&SessionContainer> { ... }

    /// Is any container running?
    pub fn has_running(&self) -> bool { ... }
}
```

### Update all callers of DiscoveredSession

Every `match` on DiscoveredSession in main.rs needs updating:
- `Stopped { .. }` and `Running { .. }` → `Active { containers, .. }` with filtering
- The `cmd_start` match becomes: check if active container exists for the requested role

## TDD Plan

```rust
#[test]
fn discover_active_with_multiple_containers() {
    // Given: work=stopped, reconcile=running
    // Active variant contains both
    // active_container() returns reconcile
}

#[test]
fn discover_volumes_only_when_no_containers() {
    // Given: volumes exist, no containers
    // Returns VolumesOnly
}

#[test]
fn discover_backwards_compat_single_container() {
    // Given: only claude-session-ctr-foo exists (old naming)
    // Returns Active with one container, role=Work
}

#[test]
fn container_by_role() {
    // Given: Active with work + reconcile
    // container(Reconcile) returns the reconcile one
    // container(Fork("x")) returns None
}
```

## Files to modify

- `src/types/session.rs` — collapse Stopped/Running into Active
- `src/lifecycle/mod.rs` — add `list_session_containers()`
- `src/session/mod.rs` — update `discover()` to use list
- `src/main.rs` — update ALL `match discovered { Stopped/Running }` patterns
- `src/render.rs` — update `session_info()` to show container tree

## Acceptance criteria

- `session show` lists all containers with their roles and states
- `ls` shows session with container count
- Existing single-container sessions appear as `Active { containers: [Work] }`
- No behavior changes to start/stop/pull/push
