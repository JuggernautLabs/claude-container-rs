# Implementation Status

Last updated: 2026-03-22

## Legend
- ✓ = implemented
- ✗ = not implemented
- Preview = read-only plan/inspection
- Execute = mutates state
- Tested = verified against real Docker/git

| Path | Preview | Execute | Tested | Notes |
|------|---------|---------|--------|-------|
| **Docker** | | | | |
| Check Docker available | ✓ `check_docker()` | ✓ returns `DockerState` | ✗ | |
| Validate image binaries | ✓ `validate_image()` | ✓ runs container, caches | ✗ | Cache by image ID SHA |
| Build image from Dockerfile | ✗ | ✓ `build_image()` via tar+bollard | ✗ | No preview (always safe) |
| Inspect container | ✗ | ✓ `inspect_container()` → `ContainerState` | ✗ | |
| Check container staleness | ✓ `check_container()` → 6 checks | ✓ returns `ContainerCheck` | ✗ | Pure inspect, no mutation |
| Create container | ✓ via `plan_launch()` | ✗ stub | ✗ | `ContainerCreateArgs` defined |
| Remove container | ✗ | ✓ `remove_container()` | ✗ | No preview/confirm gate! |
| Start stopped container | ✗ | ✓ `start_container()` | ✗ | |
| Attach to running container | ✗ | ✗ | ✗ | stdin/stdout passthrough needed |
| Launch flow (plan_launch) | ✓ returns `Plan<ContainerPlan>` | ✗ stub | ✗ | |
| **Volumes** | | | | |
| Check volumes exist | ✓ `check_volumes()` | ✓ returns `SessionVolumes` | ✗ | |
| Create volumes | ✗ | ✓ `create_volumes()` | ✗ | Idempotent |
| Read volume content | ✗ | ✗ | ✗ | `SessionVolumeContent` type exists but no reader |
| Repair volume permissions | ✗ | ✗ | ✗ | |
| **Token** | | | | |
| Find token (env/file/keychain) | ✗ | ✗ | ✗ | `TokenState` type exists |
| Inject token as mount | ✗ | ✓ `inject_token()` | ✗ | Cleans stale dirs |
| **Session** | | | | |
| Discover session state | ✗ | ✓ `discover()` → `DiscoveredSession` | ✗ | Checks volumes + container |
| Load metadata (.env) | ✗ | ✓ `load_metadata()` | ✗ | |
| Save metadata (.env) | ✗ | ✓ `save_metadata()` | ✗ | |
| Read config from volume | ✗ | ✓ `read_config()` → `SessionConfig` | ✗ | docker cat + serde_yaml |
| Discover repos in directory | ✗ | ✓ `discover_repos()` | ✗ | Filesystem scan |
| Resolve main project | ✗ | ✓ `resolve_main_project()` | ✗ | 4-tier priority |
| Create session | ✓ `plan_create()` | ✗ stub | ✗ | |
| Delete session | ✗ | ✗ | ✗ | |
| Add repo to session | ✗ | ✗ | ✗ | |
| **Sync/Snapshot** | | | | |
| Snapshot container state | ✗ | ✓ `snapshot()` one docker run | ✗ | Parses name\|head\|dirty\|... |
| Classify repo pair | ✗ | ✓ `classify_repo()` → `RepoPair` | ✗ | Computes ancestry+content+squash |
| Check ancestry (git2) | ✗ | ✓ `check_ancestry()` → `Ancestry` | ✗ | |
| Compute diff (git2) | ✗ | ✓ `compute_diff()` → `DiffSummary` | ✗ | Tree-to-tree |
| Plan sync | ✓ `plan_sync()` → `Plan<SessionSyncPlan>` | ✗ stub | ✗ | Precomputes diffs |
| **Pull (container → host)** | | | | |
| Extract (bundle + fetch) | ✗ | ✗ | ✗ | |
| Merge into target (squash) | ✗ | ✗ | ✗ | `MergeOutcome` type exists |
| Merge detection (dry-run) | ✗ | ✗ | ✗ | |
| Pull plan | ✗ | ✗ | ✗ | `PullPlan` type exists |
| **Push (host → container)** | | | | |
| Fast-forward into container | ✗ | ✗ | ✗ | |
| Push plan | ✗ | ✗ | ✗ | `PushPlan` type exists |
| **Reconcile** | | | | |
| Merge host INTO container | ✗ | ✗ | ✗ | |
| Launch Claude for conflicts | ✗ | ✗ | ✗ | |
| Post-reconcile pull back | ✗ | ✗ | ✗ | |
| **Rendering** | | | | |
| Display sync plan | ✗ | ✗ | ✗ | |
| Display pull report | ✗ | ✗ | ✗ | |
| Display push preview | ✗ | ✗ | ✗ | |
| Display session info | ✗ | ✗ | ✗ | |
| Display container check | ✗ | ✗ | ✗ | |
| **CLI wiring** | | | | |
| `claude-container sync` | ✗ | ✗ | ✗ | |
| `claude-container pull` | ✗ | ✗ | ✗ | |
| `claude-container push` | ✗ | ✗ | ✗ | |
| `claude-container session` | ✗ | ✗ | ✗ | |
| `claude-container status` | ✗ | ✗ | ✗ | |
| `claude-container -s <name>` (launch) | ✗ | ✗ | ✗ | |

## Summary

- **Types:** Complete (11 type files, ~1,300 lines)
- **Preview/inspect:** ~20 functions implemented (read-only state inspection + plan building)
- **Execute:** 0 mutation paths complete
- **Rendering:** 0 display functions
- **CLI:** skeleton only (clap parsing, not wired)
- **Tested:** 0
