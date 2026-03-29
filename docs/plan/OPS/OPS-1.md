# OPS-1: Sync Safety Foundation — Epic

## Status: COMPLETE

All active tickets done. OPS-6 through OPS-11 superseded by VM epic.

Current HEAD: 3db9d43 (committed work)
Uncommitted: OPS-2 tests, OPS-4 MergeGuard, OPS-5 clone cleanup,
plus OPS-1.5 fixes (force push, host-dirty, terminal restore, etc.)

## Tickets

| Ticket | Title | Status |
|--------|-------|--------|
| OPS-1.5 | Verify inject + idempotency + force push | **DONE** |
| OPS-2 | Test foundation: 29 scenario + merge safety tests | **DONE** |
| OPS-3 | Cleanup: inject failure (merge abort + remote remove) | **DONE** (3db9d43) |
| OPS-4 | Cleanup: MergeGuard Drop for host safety | **DONE** (uncommitted) |
| OPS-5 | Cleanup: clone pre-clean + failure cleanup | **DONE** (uncommitted) |
| OPS-6–11 | Compound Op enum, interpreter, migration | **Superseded by VM** |

## What's next

```
VM-1.5 (Backend trait skeleton, rewrite tests against it)
  │
VM-2  (12 primitive ops, RepoVM, SyncVM)
VM-3+ (mock backend, generators, real backend, migration)
```
