# Implementation Status

Last updated: 2026-03-23

## Codebase

| Component | Lines | Files |
|-----------|-------|-------|
| Type system | ~1,500 | 15 files in `src/types/` |
| Lifecycle (Docker API) | ~1,000 | `src/lifecycle/mod.rs` |
| Session (discovery, config) | ~600 | `src/session/mod.rs` |
| Sync (snapshot, classify, diff) | ~660 | `src/sync/mod.rs` |
| Container (launch, attach) | ~560 | `src/container/mod.rs` |
| Rendering | ~200 | `src/render.rs` |
| CLI | ~270 | `src/main.rs` |
| Unit tests | ~900 | `tests/types_test.rs` |
| Integration tests | ~600 | `tests/integration_test.rs` |
| **Total** | **~6,300** | |

## Tests

- **71 unit tests** — type logic, all (GitSide, GitSide) sync decision pairs, config, verified wrappers
- **12 integration tests** — Docker ops, image validation, volumes, session discovery, snapshot, classify
- **83 total, all passing**

Run: `cargo test` (unit) / `cargo test -- --ignored` (integration, needs Docker)

## Flow Status

| Flow | Preview | Execute | Tests | Status |
|------|---------|---------|-------|--------|
| **`start -s <name>`** | ✓ verified pipeline (docker→image→volumes→token→target) | ✓ create container, attach stdin/stdout, raw mode, SIGWINCH | 12 integration | **Complete — needs live test** |
| **`session show`** | ✓ discover state, read config, list repos | N/A (read-only) | 3 integration | **Complete — tested live** |
| **`sync --dry-run`** | ✓ snapshot, classify all repos, render plan | ✗ stub | 2 integration + 21 unit | **Preview complete — tested live** |
| **`status`** | routes to sync preview | ✗ | via sync | **Preview complete** |
| **`validate-image`** | ✓ one container run, cached by image ID SHA | N/A (read-only) | 3 integration | **Complete — tested live** |
| **`pull --dry-run`** | routes to sync preview | ✗ | via sync | **Preview only** |
| **`push --dry-run`** | routes to sync preview | ✗ | via sync | **Preview only** |
| **`pull` (execute)** | ✗ `PullPlan` type exists | ✗ extract (bundle+fetch), merge (squash/ff/3-way) | 0 | **Not started** |
| **`push` (execute)** | ✗ `PushPlan` type exists | ✗ inject (ff into container) | 0 | **Not started** |
| **`sync` (execute)** | ✓ plan built with precomputed diffs | ✗ `SessionSyncPlan::execute` is stub | 0 | **Not started** |
| **`session set-dir`** | ✗ | ✗ | 0 | **Not started** |
| **`session add-repo`** | ✗ | ✗ | 0 | **Not started** |
| **`session diff`** | routes to sync preview | ✗ | via sync | **Preview only** |
| **`session cleanup`** | ✗ | ✗ | 0 | **Not started** |
| **`session rebuild`** | ✗ | ✗ | 0 | **Not started** |
| **Session creation** | ✓ `plan_create` exists | ✗ stub (volumes + clone repos) | 0 | **Not started** |
| **Post-exit handling** | ✗ | ✗ read `.agent-result`, cleanup markers, merge flow | 0 | **Not started** |
| **Reconcile** | ✗ | ✗ merge host INTO container, launch Claude for conflicts | 0 | **Not started** |
| **Watch** | ✗ | ✗ poll + trigger command | 0 | **Not started** |

## Type System

Every subsystem's state is modeled as enums. Invalid states are compile errors.

| Type | Variants | Guarantees |
|------|----------|------------|
| `GitSide` | Clean, Dirty, Merging, Rebasing, NotARepo, Missing | One side of a repo |
| `RepoPair` | `(container: GitSide, host: GitSide)` | Exhaustive match on all pairs |
| `SyncDecision` | Skip, Pull, Push, Reconcile, CloneToHost, PushToContainer, Blocked | Derived from pair — compiler enforces completeness |
| `Ancestry` | Same, ContainerBehind, ContainerAhead, Diverged, Unknown | Every relationship named |
| `ContentComparison` | Identical, Different, Incomparable | Tree-level, not history |
| `SquashState` | NoPriorSquash, Active, Stale | Squash-base tracking |
| `MergeOutcome` | AlreadyUpToDate, FastForward, SquashMerge, CleanMerge, Conflict, CreateBranch, Blocked | Every merge result |
| `SessionVolumes` | Per-volume `VolumeState<Content>` with typed content | Session vs state vs cache volumes |
| `DockerState` | Available, NotInstalled, NotRunning | Docker readiness |
| `ImageState` | Valid, Invalid, NeedsBuild, NeedsRebuild, Missing | Image readiness |
| `ContainerState` | Running, Stopped, NotFound | Container lifecycle |
| `TokenState` | FromEnv, FromFile, FromKeychain, Missing | Auth token source |
| `AgentTask` | Work, ResolveConflicts, RebaseConflicts, Review, Exec | Why container launches |
| `DiscoveredSession` | DoesNotExist, VolumesOnly, Stopped, Running | Runtime discovery |
| `ContainerError` | 20+ typed variants via thiserror | Every failure mode named |

## Verified Types (proof-carrying)

Functions that require verification take `Verified<T>` wrappers. You can't skip steps.

| Proof | Means | Required by |
|-------|-------|-------------|
| `Verified<DockerAvailable>` | Docker ping succeeded | `verify_image`, `verify_volumes`, `plan_target` |
| `Verified<ValidImage>` | Image has gosu/git/claude/bash | `plan_target`, `LaunchReady` |
| `Verified<VolumesReady>` | All 5 volumes exist | `LaunchReady` |
| `Verified<TokenReady>` | Token file created | `LaunchReady` |
| `Verified<ContainerResumable>` | Staleness checks passed | `LaunchTarget::Resume` |
| `Verified<UserConfirmed>` | User approved destructive op | `LaunchTarget::Rebuild`, `RemovalApproved` |
| `LaunchReady` | ALL proofs assembled | `launch()` — the only way to start a container |

## Architecture

```
src/
  types/          — state space (enums, verified wrappers, error types)
  lifecycle/      — Docker API (bollard): image, container, volume, token
  session/        — discovery, config YAML, metadata .env, repo scanning
  sync/           — snapshot (one docker run), classify (git2), diff, plan
  container/      — verified launch pipeline, terminal attach (crossterm)
  render.rs       — colored terminal output
  main.rs         — CLI (clap) → module calls
  lib.rs          — re-exports for integration tests

tests/
  types_test.rs       — 71 unit tests (no Docker)
  integration_test.rs — 12 tests against real Docker + sessions

docs/
  GITOPS.md           — extract/merge/inject semantics
```

## Dependencies

| Crate | Purpose |
|-------|---------|
| bollard | Docker API |
| git2 | Git operations (libgit2) |
| clap | CLI parsing |
| serde + serde_yaml + serde_json | Config serialization |
| tokio | Async runtime |
| anyhow + thiserror | Error handling |
| crossterm | Terminal raw mode |
| signal-hook | SIGWINCH forwarding |
| colored | Terminal colors |
| sha2 + hex | Image validation cache keys |
| dirs | Home/config directory resolution |
| tempfile | Temporary files |
| tar | Docker image build context |
| libc | getuid/getgid |
| futures-util | Stream consumption |
