# MIRROR-1: Branch Mirroring Epic

## Goal

Make the container the source of truth for branch state. The host mirrors whatever branches the container has — creates, updates, deletes, rewrites — with user approval gates.

## Core Concepts

**Tracked branches**: per-session list of branch names to mirror between container and host. Set via `session track-branches`. Default: just the session branch (current behavior). With `--all`: every branch in the container.

**Mirror pull**: `pull -s foo` copies the container's branch state to the host for all tracked branches. Not a merge — a mirror. The host branch pointer moves to match the container's.

**`--all` mode**: the container is fully authoritative. Branches deleted in the container get deleted on host. History rewrites accepted. No approval prompts.

## Dependency DAG

```
MIRROR-2 (track-branches command) ──┐
                                    ├─→ MIRROR-4 (mirror pull)
MIRROR-3 (branch snapshot) ─────────┤
                                    ├─→ MIRROR-5 (--all / destructive sync)
                                    │
                                    └─→ MIRROR-6 (push branches into container)
```

## Phases

**Phase 1 (foundation):** MIRROR-2, MIRROR-3
**Phase 2 (pull):** MIRROR-4, MIRROR-5
**Phase 3 (push):** MIRROR-6
