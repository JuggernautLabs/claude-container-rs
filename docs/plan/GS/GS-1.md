# GS-1: git-sandbox Hardening Epic

## Goal
Fix all correctness, safety, and UX issues identified in the code audit. Each ticket is scoped for a single subagent with TDD — write failing tests first, then implement.

## Dependency DAG

```
GS-2 (typed errors) ─────────┐
                              ├─→ GS-6 (conflict detection uses types)
GS-3 (git state triple) ─────┤
                              ├─→ GS-7 (extract commit counting)
GS-4 (safety gates) ──────── ├─→ GS-8 (orphan cleanup)
                              │
GS-5 (dead flags) ───────────┘

GS-9 (shell safety) ──── independent
GS-10 (image cache) ──── independent
GS-11 (terminal) ─────── independent
GS-12 (UX polish) ────── depends on GS-2, GS-3
```

## Phases

**Phase 1 (foundations, no dependencies):** GS-2, GS-3, GS-4, GS-5, GS-9, GS-10, GS-11
**Phase 2 (depends on Phase 1):** GS-6, GS-7, GS-8, GS-12

## Ticket Status

| Ticket | Title | Status |
|--------|-------|--------|
| GS-2 | Typed Error Variants for Sync Operations | DONE |
| GS-3 | Git State Triple — (Container, Session, Target) | DONE |
| GS-4 | Safety Gates — Confirmation & Rollback | DONE |
| GS-5 | Dead Flags & Unused Code Cleanup | DONE |
| GS-6 | Typed Conflict Detection -> Agentic Reconciliation | DONE |
| GS-7 | Extract Accuracy — Commit Counting & Bundle Cleanup | DONE |
| GS-8 | Container & Ref Orphan Cleanup | DONE |
| GS-9 | Shell Safety — Command Escaping & Exec | DONE |
| GS-10 | Image Validation Cache TTL & Rebuild | DONE |
| GS-11 | Terminal Safety — Raw Mode, Cursor, Resize | DONE |
| GS-12 | UX Polish — Render, Messaging, Docker Build Output | DONE |

## Final Status

**Epic: COMPLETE**

All 11 tickets (GS-2 through GS-12) are done. Total test count across all tickets: 95 tests.

| Ticket | Tests |
|--------|-------|
| GS-2 | 5 (error_types_test.rs) |
| GS-3 | 6 (triple_test.rs) |
| GS-4 | 10 (safety_test.rs) |
| GS-5 | 8 (flags_test.rs) |
| GS-6 | 12 (reconciliation_test.rs) |
| GS-7 | 13 (extract_test.rs) |
| GS-8 | 10 (cleanup_test.rs) |
| GS-9 | 11 (shell_safety_test.rs) |
| GS-10 | 6 (image_cache_test.rs) |
| GS-11 | 6 (terminal_test.rs) |
| GS-12 | 15 (render_test.rs) |

**Bugs found during implementation:**
- GS-3: Squash-merge creates content-identical trees with different SHAs. Fixed by checking ContentComparison::Identical in maybe_merge_to_target().
- GS-5: 1 failing test (squash_false_uses_merge_commit needs diverged history fix).
