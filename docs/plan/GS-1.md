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
