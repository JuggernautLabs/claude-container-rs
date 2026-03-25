# TREE-7: Type Hygiene — Newtypes, Stringly-Typed Cleanup

blocked_by: []
unlocks: []

## Problem

Several domain concepts flow through the system as raw `String` or `&str`, making it possible to pass a repo name where a branch is expected, or a regex where a name is expected. The compiler can't catch these swaps.

## Audit: Stringly-Typed Fields

| Field | Current | Used in | Should be |
|-------|---------|---------|-----------|
| `SessionSyncPlan.target_branch` | `String` | plan_sync, merge, render | `BranchName` |
| `RepoPair.name` | `String` | classify, render | `RepoName` |
| `VolumeRepo.name` | `String` | snapshot, plan_sync | `RepoName` |
| `RepoSyncAction.repo_name` | `String` | plan, render, execute | `RepoName` |
| `RepoSyncResult::*.repo_name` | `String` | execute, render | `RepoName` |
| `AgentRepoResult.name` | `String` | agent result parsing | `RepoName` |
| `BinaryCheck.name` | `String` | image validation | `BinaryName` |
| `ContainerInspect.user` | `String` | staleness check | Could be `RunAs` but Docker returns arbitrary strings |
| `ContainerInspect.created` | `String` | display only | Fine — Docker format |
| CLI `filter: Option<String>` | `String` | build_sync_plan | `RepoFilter` (compiled regex) |
| CLI `-c` container flag | `Option<String>` | TREE-5 | `ContainerRole` via ValueEnum |

## New Types

### `BranchName` (`src/types/ids.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BranchName(String);

impl BranchName {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
}

impl fmt::Display for BranchName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { write!(f, "{}", self.0) }
}
```

### `RepoName` (`src/types/ids.rs`)

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RepoName(String);

impl RepoName {
    pub fn new(s: impl Into<String>) -> Self { Self(s.into()) }
    pub fn as_str(&self) -> &str { &self.0 }
    /// The leaf name (after last `/`): "hypermemetic/synapse" → "synapse"
    pub fn leaf(&self) -> &str { self.0.rsplit('/').next().unwrap_or(&self.0) }
}
```

### `RepoFilter` (`src/types/ids.rs` or separate)

```rust
pub struct RepoFilter(regex::Regex);

impl RepoFilter {
    pub fn new(pattern: &str) -> Result<Self, regex::Error> {
        Ok(Self(regex::Regex::new(pattern)?))
    }
    pub fn matches(&self, name: &RepoName) -> bool {
        self.0.is_match(name.as_str())
    }
}
```

Constructed at CLI boundary (clap parse or immediately after). Never a raw string inside the engine.

### `SyncableRepo` — the missing domain object

```rust
/// A repo that exists in both the container and on the host.
/// This is the unit of sync — everything operates on these.
pub struct SyncableRepo {
    pub name: RepoName,
    pub host_path: PathBuf,
    pub container_head: CommitHash,
    pub session_head: Option<CommitHash>,
    pub target_head: Option<CommitHash>,
    pub container_dirty: u32,
    pub container_merging: bool,
    pub host_dirty: bool,
}
```

Today this data is split across `VolumeRepo` (container side), `RepoPair` (combined), and `RepoSyncAction` (planned action). `SyncableRepo` is the canonical "everything we know about this repo" — `RepoPair` becomes a view over it with computed relations.

## Migration Path

### Phase A: Newtypes (mechanical, safe)

1. Add `BranchName`, `RepoName`, `RepoFilter` to `src/types/ids.rs`
2. Replace `String` fields in `RepoPair.name`, `VolumeRepo.name`, `RepoSyncAction.repo_name` with `RepoName`
3. Replace `target_branch: String` in `SessionSyncPlan` with `BranchName`
4. Replace `filter: Option<&str>` in `build_sync_plan` with `Option<RepoFilter>`
5. Fix all callers (`.as_str()` where needed, `RepoName::new()` at boundaries)

### Phase B: SyncableRepo (structural)

1. Add `SyncableRepo` struct
2. `snapshot()` returns `Vec<SyncableRepo>` instead of `Vec<VolumeRepo>`
3. `classify_repo()` takes `&SyncableRepo` instead of separate VolumeRepo + host_path
4. `RepoPair` wraps `SyncableRepo` + computed relations

## TDD Plan

```rust
#[test]
fn branch_name_prevents_repo_name_swap() {
    let branch = BranchName::new("main");
    let repo = RepoName::new("hypermemetic/synapse");
    // These are different types — can't pass one where the other is expected
    // fn merge(target: &BranchName, repo: &RepoName) — compiler enforces
}

#[test]
fn repo_name_leaf() {
    assert_eq!(RepoName::new("hypermemetic/synapse").leaf(), "synapse");
    assert_eq!(RepoName::new("synapse").leaf(), "synapse");
}

#[test]
fn repo_filter_matches() {
    let f = RepoFilter::new("gamma|synapse").unwrap();
    assert!(f.matches(&RepoName::new("hypermemetic/plexus-gamma")));
    assert!(f.matches(&RepoName::new("hypermemetic/synapse")));
    assert!(!f.matches(&RepoName::new("hypermemetic/plexus-core")));
}

#[test]
fn repo_filter_invalid_regex_fails_at_construction() {
    assert!(RepoFilter::new("[invalid").is_err());
}
```

## Files to modify

- `src/types/ids.rs` — add `BranchName`, `RepoName`, `RepoFilter`
- `src/types/git.rs` — `RepoPair.name` → `RepoName`
- `src/types/action.rs` — `RepoSyncAction.repo_name` → `RepoName`, `target_branch` → `BranchName`
- `src/types/volume.rs` — `VolumeRepo.name` → `RepoName`
- `src/types/agent.rs` — `AgentRepoResult.name` → `RepoName`
- `src/sync/mod.rs` — all functions use newtypes instead of `&str`
- `src/main.rs` — construct `RepoFilter` at CLI boundary, `BranchName` from clap args
- `src/render.rs` — `.as_str()` where display needs string

## Acceptance criteria

- Zero raw `String` for repo names, branch names, or regex filters in the sync engine
- Passing a `RepoName` where a `BranchName` is expected → compile error
- `RepoFilter` validates regex at construction, not deep in `build_sync_plan`
- All existing tests pass (mechanical refactor, no behavior change)
