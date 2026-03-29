# Agent DSL — Design Progression

How the sync engine's execution model evolved from implicit dispatch to
a formal operation language, and where it points.

## Layer 0: What existed (SyncDecision)

One enum, one answer per repo. `RepoPair.sync_decision()` collapsed
observation and action into a single value:

```rust
enum SyncDecision {
    Skip, Pull, Push, Reconcile, CloneToHost, PushToContainer,
    MergeToTarget, Blocked,
}
```

Consumers matched on it, filtering by direction. Push ignored
MergeToTarget, pull ignored Push. The renderer and executor both
switched on the same enum.

**The bug**: after squash-merge, container==session (Skip from extraction
perspective) but target had external work (Push from merge perspective).
SyncDecision picked MergeToTarget and push saw "unchanged." One slot,
two directions, wrong answer.

## Layer 1: Two-leg observation (RepoState)

Separate what IS from what to DO. Two independent observations:

```rust
struct RepoState {
    extraction: LegState,    // container ↔ session (8 variants)
    merge: MergeLeg,         // session ↔ target (6 variants)
    blocker: Option<Blocker>,
}
```

Each direction derives its own typed action:

```rust
RepoState.pull_action() → PullAction  // 6 variants, compiler-checked
RepoState.push_action() → PushAction  // 4 variants, compiler-checked
```

Push can't produce MergeToTarget. Pull can't produce Inject. The
compiler prevents the bug class.

**Dispatch**: `execute_push()` matches on PushAction (4 arms).
`dispatch_pull()` matches on PullAction (6 arms). No string-typed
direction parameter.

**What this fixed**: the squash-push bug. InSync + TargetAhead correctly
produces PullAction::MergeToTarget AND PushAction::Inject — both
visible, neither suppressed.

**What this didn't fix**: ops are still implicit method bodies.
Dispatch is still imperative: match on variant, call functions in
sequence, handle errors ad-hoc. No preview of the full execution
plan. No cleanup guarantees.

## Layer 2: Programs as data (OPS epic)

Recognize that each dispatch arm is really a small program — a
sequence of atomic operations:

```
PullAction::Extract     → [extract, merge]
PullAction::Reconcile   → [inject, extract, merge]
PushAction::Inject      → [inject]
```

Make the sequence explicit:

```rust
enum Op {
    Extract { repo },
    Inject { repo, branch },
    Merge { repo, from, to, squash },
    CloneIntoVolume { repo },
    MergeIntoVolume { repo, branch },
    ReObserve,
    LaunchReconciliation { repo, conflicts },
    Confirm { message },
}

struct SyncProgram { ops: Vec<Op> }
```

High-level commands generate programs:
```rust
SyncProgram::for_push(plan) → [Inject, Inject, ...]
SyncProgram::for_pull(plan) → [Extract, Extract, ReObserve, Merge, Merge, ...]
SyncProgram::for_sync(plan) → [push ops..., pull ops...]
```

An interpreter walks the ops. Preview renders them as numbered steps.
Each op has a compensating action for cleanup on failure (inject aborts
merge, merge has a Drop guard, clone removes partial directory).

**What this fixes**: preview shows the full plan including agent steps.
Cleanup is per-op, not scattered across callers. Programs are testable
as data — assert on the generated op sequence without running anything.

**What it doesn't fix**: ops are fire-and-forget. Extract produces a
result, but Merge doesn't receive it — it re-reads from git. State
flows through the filesystem, not through the program.

## Layer 3: VM with state transitions (VM epic)

Make state explicit. The VM holds the world:

```rust
struct SyncVM {
    repos: BTreeMap<String, RepoVM>,
    backend: Box<dyn Backend>,
}

struct RepoVM {
    container: RefState,      // At(hash) | Absent
    session: RefState,
    target: RefState,
    container_clean: bool,
    host_clean: bool,
    conflict: ConflictState,  // Clean | Markers(files) | Resolved
}
```

Ops are typed transitions: `(RepoVM, Input) → (RepoVM, Output)`.
Preconditions checked against VM state. Postconditions update VM.
Backend makes it real (docker/git2). If backend fails, VM state
unchanged — transactional.

Three backends: RealBackend (docker+git2), MockBackend (unit tests),
DryRunBackend (preview with predicted results).

**What this fixes**: testability without Docker (mock backend). Output
of one op feeds into the next (data flow, not filesystem flow).
ReObserve disappears — extract's output tells merge what session HEAD
is. Transactional: failed ops don't corrupt VM state.

## Layer 4: Abstract primitives (future)

The VM ops decompose into 12 irreducible primitives in 4 categories:

**State** — read/write named references in an environment:
```
read(env, ref)        → Value
write(env, ref, val)  → ()
compare(env, a, b)    → Comparison
```

**Content** — manipulate work product:
```
combine(env, a, b)    → CombineResult { merged | conflicted(regions) }
snapshot(env)         → Snapshot
apply(env, snapshot)  → ()
```

**Transport** — move work between environments:
```
export(from_env, scope)  → Bundle
import(to_env, bundle)   → ()
clone(from_env, to_env)  → ()
```

**Agency** — agents act within environments:
```
invoke(env, agent, task) → AgentResult
gate(actor, prompt)      → Decision
```

"Environment" replaces "container/host/session" — it's an isolated
workspace where work happens. git-sandbox has three (container,
session branch, target branch). The primitives don't mention git or
docker.

Agent invocation is first-class, same rank as combine or transport.
Claude resolving conflicts is `invoke(container, claude, resolve)`.
User confirming a merge is `gate(user, "proceed?")`. CI approving
a deploy could be `gate(ci, "tests pass?")`.

The programs from Layer 2 rewrite cleanly:

```
pull = export(container) → import(session) → write(session, HEAD)
     → snapshot(session)
     → combine(target, session, target) → apply(target) → write(target, HEAD)

push = export(target) → import(container)
     → combine(container, container, imported) → apply(container)

reconcile(conflicted) =
     apply(container, conflict_markers)
     → invoke(container, claude, resolve_conflicts)
     → export(container) → import(session)
     → combine(target, session, target) → apply(target)
```

**What this would fix**: the language is git-agnostic. Same primitives
could coordinate file-system snapshots, database migrations, cloud
workspaces. Agent invocation and user gates use the same mechanism.
Programs are composable from universal building blocks.

**Why we're not building this now**: it's a research direction, not
a product need. Layers 1-3 solve the real bugs and give us testable,
safe, previewable sync. Layer 4 is documented here so we don't lose
the insight, but we build from Layer 1 up, not from Layer 4 down.

## Current state

Layer 1 is implemented and deployed. Layer 2 and 3 are planned
(docs/plan/OPS/ and docs/plan/VM/). Layer 4 is this document.

## Implementation order

```
Layer 1 (done)  → fix the squash-push bug, typed dispatch
Layer 2 (next)  → OPS-2..5 bug fixes, then OPS-6..11 program abstraction
Layer 3 (after) → VM-2..7 state machine with backends
Layer 4 (future)→ abstract primitives, if the pattern proves out
```

Each layer subsumes the previous. Layer 2 programs are sequences of
Layer 1 dispatch calls. Layer 3 VM executes Layer 2 programs with
explicit state. Layer 4 decomposes Layer 3 ops into universal
primitives. You can stop at any layer and have a working system.
