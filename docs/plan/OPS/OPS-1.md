# OPS-1: Sync Operation Language — Epic Overview

## Goal

Formalize the sync engine's implicit workflow language into an explicit,
data-driven execution model. The engine already implements a tiny DSL —
primitives, composition, control flow, agentic gates — but it's expressed
as method bodies rather than data. This epic makes the language explicit.

## What Exists Today

### Three Operation Layers

**Layer 1 — Git Ops** (5 side-effecting primitives):
- `extract` — container→host via bundle
- `inject` — host→container via fetch+merge in throwaway container
- `merge` — host-local branch merge (FF/squash/regular)
- `clone_into_volume` — first-time host→container clone
- `merge_into_volume` — host→container merge preserving conflict markers

**Layer 2 — Observation Ops** (4 read-only):
- `snapshot` — scan container volume
- `classify` — two-leg state observation (RepoState)
- `diff` — tree comparison between commits
- `trial_merge` — in-memory conflict prediction

**Layer 3 — Agentic & Interactive Ops** (4):
- `launch_reconciliation` — start container with Claude, wait for resolution
- `confirm` — user Y/N gate
- `offer_choice` — user multi-option gate (auto/skip/reconcile)
- `re_plan` — re-observe after partial execution (the plan loop)

### Current Programs (implicit)

```
pull      = extract → merge
push      = inject
sync      = [all inject] → [all extract → merge]
reconcile = inject → merge_into_volume → launch_claude → extract → merge
clone     = extract (first time)
```

### Control Flow (implicit in method bodies)

- **Branch on observation**: trial_merge result selects auto vs agentic path
- **User gate**: confirm/offer_choice pauses for input
- **Agent gate**: launch_reconciliation blocks until Claude exits
- **Re-plan loop**: extract → re-snapshot → re-classify → merge

### Type Safety (what the compiler enforces)

- `PushAction` (4 variants) — can't merge, can't extract
- `PullAction` (6 variants) — can't inject (except Reconcile's first step)
- `RepoState` observation is pure — derived from RepoPair, no side effects
- Dispatch is exhaustive — compiler catches missing arms

### Gaps (what the types DON'T enforce)

- Re-plan loop is manual (`cmd_pull` calls `plan_sync` twice)
- Agentic ops are outside the dispatch system
- Phase ordering (push before pull) is convention, not type
- Conflict branching is imperative code, not declarative program

## Dependency DAG

```
OPS-2 (formalize primitives) ──────┐
                                    ├─→ OPS-5 (program-as-data)
OPS-3 (formalize observations) ────┤
                                    ├─→ OPS-6 (preview full DAG)
OPS-4 (formalize agentic ops) ─────┘
                                         │
                                         ├─→ OPS-7 (interpreter)
                                         └─→ OPS-8 (serialize/resume)
```

## Tickets

| Ticket | Scope | Blocked by |
|--------|-------|------------|
| OPS-2 | Define `GitOp` enum — the 5 primitives as data | — |
| OPS-3 | Define `ObserveOp` — snapshot/classify/diff/trial_merge as data | — |
| OPS-4 | Define `AgentOp` — launch_reconciliation, confirm, re_plan as data | — |
| OPS-5 | Define `Program` — DAG of ops with typed branches and gates | OPS-2,3,4 |
| OPS-6 | Preview renderer — display full execution DAG before running | OPS-5 |
| OPS-7 | Interpreter — execute a Program against SyncEngine | OPS-5 |
| OPS-8 | Serialize/resume — persist program state across container restarts | OPS-7 |

## Design Principle

Programs are **data, not code**. You build a program (a typed DAG of
operations), preview it (render every step including "Claude will resolve
conflicts here"), then execute it (an interpreter walks the DAG). This
enables dry-run of the entire workflow including agentic steps, structured
logging of each step, and resuming interrupted workflows.
