# OPS-1: Sync Safety Foundation — Epic

## Status

OPS-2 through OPS-5 are active. OPS-6 through OPS-11 are superseded
by the VM epic (docs/plan/VM/). The compound Op enum + interpreter
is skipped — we go straight from cleanup fixes to primitive-level VM.

## What's here

| Ticket | Title | Status |
|--------|-------|--------|
| OPS-1.5 | Verify inject works end-to-end | Active — do first |
| OPS-2 | Test foundation: derivation + merge safety | Active |
| OPS-3 | Cleanup: inject failure leaves container dirty | Active |
| OPS-4 | Cleanup: merge crash leaves host dirty | Active |
| OPS-5 | Cleanup: partial clone leaves stale directory | Active |
| OPS-6–11 | Compound Op enum, interpreter, migration | **Superseded by VM** |

## Execution order

```
OPS-1.5 (verify inject works — do first, may reveal real bug)
  │
OPS-2  (test foundation — pin behavior)
  │
OPS-3 ── OPS-4 ── OPS-5  (cleanup fixes — parallel)
  │
VM-2   (primitive VM types — see docs/plan/VM/VM-1.md)
VM-3+  (mock backend, generators, real backend, migration)
```

OPS-2 through OPS-5 are Phase 0 and Phase 1 of the VM plan.
After they land, work continues in the VM epic.
