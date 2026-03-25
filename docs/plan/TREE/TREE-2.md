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

### ParentRef — proof of lineage (`src/types/docker.rs`)

A container knows who spawned it. The type encodes whether the parent still exists — not as a string check, but as an enum variant that can only transition through `resolve()`.

```rust
/// Tracks a container's parent with proof-of-existence.
///
/// Lifecycle: Root (never changes)
///            Alive(name) → Orphaned(name) after resolve() finds parent gone
///            Orphaned(name) → Alive(name) if parent is recreated (rebuild)
#[derive(Debug, Clone, PartialEq)]
pub enum ParentRef {
    /// Root container — no parent by design.
    Root,
    /// Parent exists — we verified it (at spawn time or last resolve).
    Alive(ContainerName),
    /// Parent existed when we spawned, but it's gone now.
    /// The name is preserved for display and re-linking after rebuild.
    Orphaned(ContainerName),
}

impl ParentRef {
    /// Set at container creation — parent must exist (we're spawning from it).
    pub fn spawned_by(parent: ContainerName) -> Self {
        Self::Alive(parent)
    }

    /// Re-check: is the parent still in the tree?
    /// Alive → Orphaned if gone. Orphaned → Alive if recreated.
    pub fn resolve(&self, tree: &[ContainerNode]) -> Self {
        match self {
            Self::Root => Self::Root,
            Self::Alive(name) | Self::Orphaned(name) => {
                if tree.iter().any(|n| &n.name == *name) {
                    Self::Alive(name.clone())
                } else {
                    Self::Orphaned(name.clone())
                }
            }
        }
    }

    pub fn name(&self) -> Option<&ContainerName> {
        match self {
            Self::Root => None,
            Self::Alive(n) | Self::Orphaned(n) => Some(n),
        }
    }

    pub fn is_orphaned(&self) -> bool { matches!(self, Self::Orphaned(_)) }
}
```

Why not `Weak<T>`: Docker containers aren't Rust heap objects. We want the name to survive deletion (for display, for re-linking after rebuild). The proof is in the enum variant, not in whether a pointer dereferences.

### ContainerNode — the tree node (`src/types/docker.rs`)

```rust
#[derive(Debug, Clone)]
pub struct ContainerNode {
    pub role: ContainerRole,
    pub name: ContainerName,
    pub state: ContainerState,
    pub info: Option<ContainerInspect>,
    pub parent: ParentRef,
}

impl ContainerNode {
    pub fn is_root(&self) -> bool { self.parent == ParentRef::Root }
}
```

The tree is implicit in the `parent` pointers:
```
ContainerNode { role: Work, parent: Root }
ContainerNode { role: Reconcile, parent: Alive("claude-session-ctr-hypno") }
ContainerNode { role: Fork("gpu"), parent: Alive("claude-session-ctr-hypno") }
```

Stored as a Docker label on each container: `claude-container.parent=claude-session-ctr-hypno`. Recovered during discovery. Root containers have no label (or label = "root").

### How lineage is used

| Scenario | ParentRef tells us | Action |
|----------|-------------------|--------|
| Reconcile finishes | `Alive(work)` → parent exists | Restart work |
| Reconcile finishes | `Orphaned(work)` → parent gone | Warn, don't restart |
| Fork deleted | `Alive(work)` | Nothing — don't touch parent |
| `session show` | Tree structure | Indent children under parent |
| Volume exclusivity | Parent is running | Must stop parent or use --force |

### Future: ContainerHandle pattern

The `ContainerNode` is data — it stores state but doesn't manage it. A future upgrade wraps Docker calls so operations update state and produce typed outcomes:

```rust
/// Wraps a ContainerNode with Docker API access.
/// Operations update internal state and return what should happen next.
struct ContainerHandle {
    node: ContainerNode,
}

/// What should happen after a container operation.
enum Outcome {
    Nothing,
    RestartParent,   // reconcile finished → restart work
    Cleanup,         // runner finished → remove container
    Orphaned,        // parent disappeared during operation
}

impl ContainerHandle {
    fn stop(&mut self, docker: &Docker) -> Result<Outcome> {
        docker.stop_container(&self.node.name, ...)?;
        self.node.state = ContainerState::Stopped;

        match self.node.role {
            ContainerRole::Reconcile => Ok(Outcome::RestartParent),
            ContainerRole::Runner => Ok(Outcome::Cleanup),
            _ => Ok(Outcome::Nothing),
        }
    }

    fn start(&mut self, docker: &Docker) -> Result<Outcome> {
        docker.start_container(&self.node.name, ...)?;
        self.node.state = ContainerState::Running;
        Ok(Outcome::Nothing)
    }

    /// If a Docker call fails with "not found", transition parent to Orphaned.
    fn handle_not_found(&mut self, tree: &[ContainerNode]) -> Outcome {
        self.node.parent = self.node.parent.resolve(tree);
        if self.node.parent.is_orphaned() {
            Outcome::Orphaned
        } else {
            Outcome::Nothing
        }
    }
}
```

This is NOT in scope for TREE-2. It's documented here as the natural evolution:
- **TREE-2**: `ContainerNode` + `ParentRef` (data + proof)
- **Future**: `ContainerHandle` (data + proof + operations + state transitions)

## TDD Plan

```rust
// --- ContainerRole + naming ---

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

// --- ParentRef lifecycle ---

#[test]
fn root_never_changes() {
    let root = ParentRef::Root;
    assert_eq!(root.resolve(&[]), ParentRef::Root);
    assert_eq!(root.name(), None);
}

#[test]
fn alive_stays_alive_when_parent_exists() {
    let parent_name = ContainerName::new("claude-session-ctr-hypno");
    let alive = ParentRef::spawned_by(parent_name.clone());
    let tree = vec![ContainerNode {
        role: ContainerRole::Work,
        name: parent_name.clone(),
        state: ContainerState::Running { .. },
        info: None,
        parent: ParentRef::Root,
    }];
    assert_eq!(alive.resolve(&tree), ParentRef::Alive(parent_name));
}

#[test]
fn alive_becomes_orphaned_when_parent_gone() {
    let parent_name = ContainerName::new("claude-session-ctr-hypno");
    let alive = ParentRef::spawned_by(parent_name.clone());
    let empty_tree: Vec<ContainerNode> = vec![];
    assert_eq!(alive.resolve(&empty_tree), ParentRef::Orphaned(parent_name.clone()));
    // Name preserved even after orphaning
    assert_eq!(alive.resolve(&empty_tree).name(), Some(&parent_name));
}

#[test]
fn orphaned_becomes_alive_when_parent_recreated() {
    let parent_name = ContainerName::new("claude-session-ctr-hypno");
    let orphaned = ParentRef::Orphaned(parent_name.clone());
    let tree = vec![ContainerNode {
        role: ContainerRole::Work,
        name: parent_name.clone(),
        state: ContainerState::Stopped { .. },
        info: None,
        parent: ParentRef::Root,
    }];
    // Parent rebuilt → back to Alive
    assert_eq!(orphaned.resolve(&tree), ParentRef::Alive(parent_name));
}

#[test]
fn spawned_by_is_only_constructor_for_alive() {
    // Can't construct Alive directly — must go through spawned_by()
    // (Alive is a variant, not pub(crate) constructible from outside)
    let p = ParentRef::spawned_by(ContainerName::new("x"));
    assert!(!p.is_orphaned());
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
