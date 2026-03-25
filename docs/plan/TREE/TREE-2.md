# TREE-2: ContainerRole Enum + Naming

blocked_by: []
unlocks: [TREE-3, TREE-4, TREE-5]

## Problem

`SessionName::container_name()` returns exactly one name: `claude-session-ctr-{session}`. There's no way to have multiple named containers per session. The role is implicit (always "work").

## Scope

### ContainerRole enum (`src/types/docker.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ContainerRole {
    /// Primary work container (the default)
    Work,
    /// Conflict resolution (from pull --reconcile)
    Reconcile,
    /// User-created fork for experimentation
    Fork(String),  // validated label: alphanumeric + hyphens only
    /// Headless task runner
    Runner,
}
```

Not a String wrapper — it's an enum. Only `Fork` carries a dynamic label, and that label is validated at construction.

### Container naming (`src/types/ids.rs`)

```rust
impl SessionName {
    pub fn container_name(&self) -> ContainerName {
        // Backwards compat — existing containers keep their name
        ContainerName(format!("claude-session-ctr-{}", self.0))
    }

    pub fn container_name_for(&self, role: &ContainerRole) -> ContainerName {
        match role {
            ContainerRole::Work => self.container_name(),
            ContainerRole::Reconcile => ContainerName(format!("claude-reconcile-ctr-{}", self.0)),
            ContainerRole::Fork(label) => ContainerName(format!("claude-fork-{}-ctr-{}", label, self.0)),
            ContainerRole::Runner => ContainerName(format!("claude-run-ctr-{}", self.0)),
        }
    }

    /// Parse a container name back into (SessionName, ContainerRole).
    /// Returns None if the name doesn't match any known pattern.
    pub fn parse_container_name(name: &str) -> Option<(SessionName, ContainerRole)> {
        if let Some(session) = name.strip_prefix("claude-session-ctr-") {
            Some((SessionName::new(session), ContainerRole::Work))
        } else if let Some(session) = name.strip_prefix("claude-reconcile-ctr-") {
            Some((SessionName::new(session), ContainerRole::Reconcile))
        } else if let Some(rest) = name.strip_prefix("claude-fork-") {
            // claude-fork-{label}-ctr-{session}
            if let Some(idx) = rest.find("-ctr-") {
                let label = &rest[..idx];
                let session = &rest[idx + 5..];
                Some((SessionName::new(session), ContainerRole::Fork(label.to_string())))
            } else { None }
        } else if let Some(session) = name.strip_prefix("claude-run-ctr-") {
            Some((SessionName::new(session), ContainerRole::Runner))
        } else { None }
    }
}
```

### SessionContainer struct (`src/types/docker.rs`)

```rust
pub struct SessionContainer {
    pub role: ContainerRole,
    pub name: ContainerName,
    pub state: ContainerState,
    pub info: Option<ContainerInspect>,
}
```

## TDD Plan

```rust
#[test]
fn work_role_produces_backwards_compat_name() {
    let s = SessionName::new("hypno");
    assert_eq!(s.container_name_for(&ContainerRole::Work).as_str(), "claude-session-ctr-hypno");
    assert_eq!(s.container_name().as_str(), s.container_name_for(&ContainerRole::Work).as_str());
}

#[test]
fn reconcile_role_produces_distinct_name() {
    let s = SessionName::new("hypno");
    assert_eq!(s.container_name_for(&ContainerRole::Reconcile).as_str(), "claude-reconcile-ctr-hypno");
}

#[test]
fn fork_role_includes_label() {
    let s = SessionName::new("hypno");
    assert_eq!(s.container_name_for(&ContainerRole::Fork("experiment".into())).as_str(), "claude-fork-experiment-ctr-hypno");
}

#[test]
fn parse_roundtrips() {
    let s = SessionName::new("hypno");
    for role in [ContainerRole::Work, ContainerRole::Reconcile, ContainerRole::Fork("gpu".into()), ContainerRole::Runner] {
        let name = s.container_name_for(&role);
        let (parsed_session, parsed_role) = SessionName::parse_container_name(name.as_str()).unwrap();
        assert_eq!(parsed_session.as_str(), "hypno");
        assert_eq!(parsed_role, role);
    }
}
```

## Files to modify

- `src/types/docker.rs` — add `ContainerRole`, `SessionContainer`
- `src/types/ids.rs` — add `container_name_for()`, `parse_container_name()`
- `src/types/mod.rs` — re-export new types

## Acceptance criteria

- `ContainerRole` is an enum with Work, Reconcile, Fork(String), Runner
- `container_name()` unchanged (backwards compat)
- `container_name_for(Work)` == `container_name()`
- `parse_container_name()` roundtrips all roles
- No behavior changes — types only
