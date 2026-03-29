# OPS-2: Test Foundation — Derivation + Merge Safety

blocked_by: []
unlocks: [OPS-6]

## Why first

Everything after this ticket changes behavior. We need tests that pin
the current behavior before we start moving things around. These tests
also cover the gap left by migrating off SyncDecision — the 24
sync_decision_tests exercise dead code.

## Scope

### Part A: Pure derivation tests (no git, no Docker)

Enumerate reachable (LegState × MergeLeg × Blocker) triples. For each,
assert the correct PullAction and PushAction.

Table-driven. One test function, parameterized over cases:

```rust
struct Case {
    name: &'static str,
    state: RepoState,
    expected_pull: PullAction,
    expected_push: PushAction,
}
```

Minimum cases to cover:

| # | Extraction | Merge | Pull | Push |
|---|---|---|---|---|
| 1 | InSync | InSync | Skip | Skip |
| 2 | InSync | SessionAhead{3} | MergeToTarget{3} | Skip |
| 3 | InSync | TargetAhead{2,false} | Skip | Inject{2} |
| 4 | InSync | TargetAhead{2,true} | Skip | Inject{2} |
| 5 | InSync | Diverged{3,2} | MergeToTarget{3} | Inject{2} |
| 6 | InSync | ContentIdentical | Skip | Skip |
| 7 | InSync | NoTarget | Skip | Skip |
| 8 | ContainerAhead{5} | InSync | Extract{5} | Skip |
| 9 | ContainerAhead{5} | TargetAhead{2,false} | Extract{5} | Inject{2} |
| 10 | SessionAhead{3} | InSync | Skip | Inject{3} |
| 11 | SessionAhead{3} | TargetAhead{2,false} | Skip | Inject{2} |
| 12 | Diverged{3,2} | InSync | Reconcile | Skip |
| 13 | Diverged{3,2} | TargetAhead{1,false} | Reconcile | Inject{1} |
| 14 | Unknown | InSync | Extract{1} | Skip |
| 15 | NoSessionBranch | InSync | CloneToHost | Skip |
| 16 | NoSessionBranch | NoTarget | CloneToHost | Skip |
| 17 | NoContainer | InSync | Skip | PushToContainer |
| 18 | ContentIdentical | InSync | Skip | Skip |
| 19 | ContentIdentical | SessionAhead{2} | MergeToTarget{2} | Skip |
| 20 | InSync | InSync + Blocker::ContainerDirty(3) | Blocked | Blocked |
| 21 | InSync | InSync + Blocker::HostDirty | Blocked | Blocked |
| 22 | ContainerAhead{1} | TargetAhead{1,false} + Blocker::ContainerMerging | Blocked | Blocked |

Plus the **squash-push regression case**:
| 23 | InSync | Diverged{4,3} | MergeToTarget{4} | Inject{3} |

This is the bug: push must produce Inject, not Skip.

### Part B: Merge safety tests (git2, no Docker)

Each test creates a real git repo, sets up branches, calls `merge()`,
asserts postconditions.

| Test | Setup | Assert |
|---|---|---|
| `merge_ff_advances_target` | session 2 ahead, FF possible | target_head = session_head |
| `merge_squash_creates_single_commit` | session 3 ahead | target has 1 new commit, tree matches session |
| `merge_squash_incremental` | squash-base exists, 2 new commits | only 2 squashed, not all |
| `merge_squash_stale_base_falls_back` | squash-base not ancestor of session | uses merge-base instead |
| `merge_conflict_rolls_back` | conflicting file on both branches | target_head unchanged |
| `merge_conflict_no_markers_on_target` | conflicting file | no `<<<<<<<` in target tree |
| `merge_conflict_worktree_clean` | conflicting file | `git status` clean after |
| `merge_already_up_to_date` | session == target | AlreadyUpToDate, no changes |
| `merge_session_behind_target` | target ahead of session | AlreadyUpToDate |
| `merge_host_dirty_fails_precondition` | uncommitted changes on host | Err, target unchanged |
| `merge_no_session_branch_fails` | session branch doesn't exist | Err, target unchanged |

### Part C: Migrate sync_decision_tests

The 24 existing `sync_decision_tests` construct RepoPair values and
assert on `sync_decision()`. Rewrite each to also assert on
`repo_state().pull_action()` and `repo_state().push_action()`.
Don't delete the old assertions yet (SyncDecision removal is GS-23).

## File locations

- `tests/two_leg_test.rs` — new file for Part A + B
- `src/types/git.rs` — existing sync_decision_tests module, add parallel assertions

## Acceptance criteria

- ≥23 pure derivation tests, all pass without Docker
- ≥11 merge safety tests, all pass without Docker
- Squash-push regression test exists and passes
- `cargo test` (no --ignored) exercises all new tests
