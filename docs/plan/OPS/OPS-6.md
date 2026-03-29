---
status: SUPERSEDED by VM epic (docs/plan/VM/VM-1.md)
---

# OPS-6: Define Op Enum + Compensating Actions

blocked_by: [OPS-3, OPS-4, OPS-5]
unlocks: [OPS-7, OPS-8, OPS-9]

## Why after cleanup tickets

OPS-3/4/5 make each atomic operation safe to fail independently. This
ticket defines the Op enum that wraps those operations. If the
operations aren't individually safe, the Op abstraction just papers
over broken primitives.

## Design

### Op enum

```rust
/// An atomic sync operation. Each variant maps to exactly one
/// SyncEngine method. Programs are Vec<Op>.
pub enum Op {
    // Git ops — side-effecting
    Extract { repo: String },
    Inject { repo: String, branch: String },
    Merge { repo: String, from_branch: String, to_branch: String, squash: bool },
    CloneIntoVolume { repo: String },
    MergeIntoVolume { repo: String, branch: String },

    // Observation — re-snapshot mid-program
    ReObserve,

    // Agent — launch Claude in container
    LaunchReconciliation { repo: String, conflicts: Vec<String> },

    // Control — user interaction
    Confirm { message: String },
}
```

### OpResult

```rust
/// What happened when an Op was executed.
pub enum OpResult {
    Extracted { repo: String, commits: u32, new_head: CommitHash },
    Injected { repo: String },
    Merged { repo: String, outcome: MergeOutcome },
    Cloned { repo: String },
    MergedIntoVolume { repo: String, had_conflicts: bool },
    ReObserved { plan: SessionSyncPlan },
    ReconciliationComplete { repo: String, description: Option<String> },
    Confirmed,
    Declined,
    Failed { repo: String, op: String, error: String },
}
```

### Compensating actions

Each Op knows how to clean up after itself. This is baked into the
interpreter (OPS-7), not the Op enum — the enum is just data. But
the design is documented here:

| Op | On failure | Compensating action |
|---|---|---|
| Extract | Bundle fetch failed | No cleanup needed (tempfile Drop) |
| Inject | Merge failed in container | Already fixed by OPS-3: abort + remove remote |
| Merge | Error at any point | Already fixed by OPS-4: MergeGuard Drop |
| CloneIntoVolume | Partial clone | Already fixed by OPS-5: rm -rf on failure |
| MergeIntoVolume | Merge failed | Intentional: leaves markers for Claude |
| ReObserve | Snapshot failed | Return error, program stops |
| LaunchReconciliation | Claude exits without resolving | Return unresolved, caller decides |
| Confirm | User declines | Return Declined, program stops |

### Where it lives

New file: `src/types/op.rs`

Exported from `src/types/mod.rs`:
```rust
pub mod op;
pub use op::*;
```

## Acceptance criteria

- `Op` enum defined with all 8 variants
- `OpResult` enum defined with all 9 variants
- Both derive Debug, Clone
- Op has Display impl (for preview rendering)
- Compiles with no new functionality yet (types only)
- Unit tests for Display formatting of each Op variant
