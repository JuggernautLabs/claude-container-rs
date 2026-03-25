# TREE-1: Session Container Tree Epic

## Goal

A session owns a tree of named containers sharing volumes. Each container has an enum-typed role, its own image/env, and safe volume access.

## Dependency DAG

```
TREE-7 (type hygiene) ─────── independent, do first

TREE-2 (types + naming) ──────┐
                               ├─→ TREE-4 (volume exclusivity)
TREE-3 (discovery) ────────────┤
                               ├─→ TREE-5 (-c flag + CLI)
                               │
                               └─→ TREE-6 (fork)
```

GS-13 (merge-into-volume) is a prerequisite for reconciliation integration but not for the tree model itself.

## Phases

**Phase 0 (type hygiene):** TREE-7 — newtypes, stringly-typed cleanup
**Phase 1 (foundations):** TREE-2, TREE-3
**Phase 2 (UX):** TREE-4, TREE-5
**Phase 3 (advanced):** TREE-6

## Tickets

| Ticket | Title | Blocked by |
|--------|-------|------------|
| TREE-7 | Type hygiene — BranchName, RepoName, RepoFilter, SyncableRepo | — |
| TREE-2 | ContainerRole enum + naming | — |
| TREE-3 | Multi-container discovery | TREE-2 |
| TREE-4 | Volume exclusivity enforcement | TREE-2, TREE-3 |
| TREE-5 | `-c` flag, session show, stop/attach per-container | TREE-3 |
| TREE-6 | Fork: volume snapshot + new container | TREE-4, TREE-5 |
