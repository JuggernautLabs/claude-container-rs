# MIRROR-3: Branch Snapshot — Read All Container Branches

blocked_by: []
unlocks: [MIRROR-4, MIRROR-5, MIRROR-6]

## Problem

The current `snapshot()` only reads HEAD per repo. To mirror branches, we need to know ALL branches in each container repo: names, HEADs, and whether they exist on the host.

## Scope

Extend the container snapshot to include branch listings.

### New data structure

```rust
pub struct RepoBranchSnapshot {
    pub name: String,          // branch name
    pub head: CommitHash,      // where it points
    pub is_head: bool,         // is this the checked-out branch?
}

// Added to existing VolumeRepo
pub struct VolumeRepo {
    pub name: String,
    pub head: CommitHash,
    pub dirty_files: u32,
    pub merging: bool,
    pub git_size_mb: u32,
    pub branches: Vec<RepoBranchSnapshot>,  // NEW
}
```

### Container scan script update

Current script outputs: `name|head|dirty|merging|rebasing|gitsize`

New script adds branch listing:
```bash
# After the existing scan line, output branches
branches=$(cd "$d" && git for-each-ref --format='%(refname:short) %(objectname:short)' refs/heads/ 2>/dev/null)
echo "$name|$head|$dirty|$merging|$rebasing|${gitsize:-0}|${branches// /,}"
```

### Host-side comparison

For each tracked branch in a container repo, check the host:

```rust
pub struct BranchComparison {
    pub name: String,
    pub container_head: CommitHash,
    pub host_head: Option<CommitHash>,  // None = doesn't exist on host
    pub relation: BranchRelation,
}

pub enum BranchRelation {
    Same,                          // identical HEADs
    ContainerAhead { commits: u32 },
    HostAhead { commits: u32 },
    Diverged { container_ahead: u32, host_ahead: u32 },
    NewOnContainer,                // container has it, host doesn't
    DeletedOnContainer,            // host has it, container doesn't
    Unknown,                       // container commit not on host
}
```

## Files to modify

- `src/types/volume.rs` — add `branches` to `VolumeRepo`, new types
- `src/sync/mod.rs` — update `SCAN_SCRIPT`, `parse_scan_output`, `snapshot()`
- `src/sync/mod.rs` — new `compare_branches()` function
