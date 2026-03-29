# OPS-2: Test Generation from TLA+ State Space

blocked_by: []
unlocks: [OPS-5, OPS-6, OPS-7]

## Problem

The TLA+ spec defines the state space and safety properties. But the spec
runs in TLC — it doesn't produce Rust test cases. We need a systematic way
to cover the state space with real tests that catch regressions.

## Three Test Layers

### Layer 1: Pure state derivation (no git, no Docker)

The `RepoState → PullAction / PushAction` derivation is a pure function.
Every (LegState × MergeLeg × Option<Blocker>) triple maps to exactly one
action per direction. We can enumerate them.

**State space**: 8 LegState variants × 6 MergeLeg variants × 6 Blocker
variants (None + 5 Some) = 288 triples. Most are unreachable (e.g.,
`NoContainer` + `SessionAhead` can't coexist). The reachable subset is
~40-60 triples.

**Test structure**: table-driven. One `#[test]` per interesting triple.

```rust
#[test]
fn pull_action_container_ahead_target_ahead() {
    let state = RepoState {
        extraction: LegState::ContainerAhead { commits: 3 },
        merge: MergeLeg::TargetAhead { commits: 2, all_squash: false },
        blocker: None,
    };
    assert_eq!(state.pull_action(), PullAction::Extract { commits: 3 });
    assert_eq!(state.push_action(), PushAction::Inject { commits: 2 });
}
```

**What this catches**: logic errors in action derivation. Wrong variant
returned for a given state. The squash-push bug would have been caught
here: `InSync + TargetAhead` must produce `PushAction::Inject`, not
`PullAction::MergeToTarget`.

**Coverage**: exhaustive over reachable state space. No Docker needed.
Runs in milliseconds.

### Layer 2: Merge safety (git2, no Docker)

The `merge()` function is the only operation that touches `target_head`.
We need to test every outcome path with actual git repos.

**Scenarios from the TLA+ spec**:

| Scenario | Setup | Expected |
|---|---|---|
| FF merge | session 2 ahead of target | target advances, no markers |
| Squash merge (first) | session 3 ahead, no squash-base | target gets squash commit, squash-base set |
| Squash merge (incremental) | session 2 new commits since squash-base | only new commits squashed |
| Squash merge (stale base) | squash-base not ancestor of session (rebased) | falls back to merge-base |
| Conflict → rollback | session and target both modified same file | target_head UNCHANGED, worktree clean, no markers |
| Already up to date | session == target | no-op |
| Session behind target | target ahead of session | AlreadyUpToDate |

**Adversarial scenarios** (user mutates between steps):

| Scenario | What user does | Expected |
|---|---|---|
| Target advances between plan and merge | user pushes to main | merge sees new target, may conflict → rollback |
| Host dirtied between plan and merge | user edits files | merge precondition fails (hDirty) |
| Session branch deleted between plan and merge | user deletes branch | merge returns error, target unchanged |
| Target force-pushed between plan and merge | user resets main | merge-base changes, may produce different result, target never gets markers |

**Test structure**: each test creates a real git repo (via `TestRepo` or
`git2::Repository::init`), sets up the specific state, calls `merge()`,
asserts on:
1. `target_head` after merge (advanced or unchanged)
2. No conflict markers in any committed tree on target
3. Working tree state (clean if merge succeeded or rolled back)
4. Squash-base ref state

```rust
#[test]
fn merge_conflict_never_commits_markers() {
    let repo = TestRepo::new("conflict-test");
    // Create divergent branches with conflicting file
    repo.commit("base", &[("shared.txt", "original")]);
    // ... create session branch, commit conflicting change
    // ... create target branch, commit different change to same file

    let engine = SyncEngine::new_local();  // no Docker needed for merge
    let result = engine.merge(&repo.path, "session", "main", true);

    assert!(matches!(result, Ok(MergeOutcome::Conflict { .. })));

    // THE SAFETY CHECK: target HEAD has no markers
    let target_tree = read_tree_at(&repo.path, "main");
    for file in target_tree {
        assert!(!file.content.contains("<<<<<<<"),
            "Conflict markers found in {} on target branch", file.path);
    }
}
```

**Coverage**: all 7 merge outcome paths, plus 4 adversarial scenarios.
No Docker needed. Runs in seconds.

### Layer 3: Full pipeline with adversarial interleaving (Docker)

End-to-end tests that exercise the full observe → plan → execute cycle
with environment mutations between steps.

**Scenarios from the TLA+ spec**:

| Scenario | Program | Environment action | Property |
|---|---|---|---|
| Push actually injects | push with TargetAhead | none | container HEAD advances |
| Push doesn't merge-to-target | push with InSync+TargetAhead | none | target unchanged, container updated |
| Pull extracts then merges | pull with ContainerAhead | none | session+target advance |
| Sync: push then pull | both directions have work | none | both sides advance |
| User pushes during sync | sync | user commits to main between push and pull phase | pull sees updated target, may skip or conflict |
| User dirties during merge | pull | user edits host files between extract and merge | merge blocked (hDirty), target unchanged |
| Re-plan convergence | pull with extract needed | none | after extract, re-plan produces merge, not another extract |
| Squash-push idempotence | push same content twice | none | second push is no-op (content identical) |

**Test structure**: uses existing `TestSession` + `seed_container_repo`
harness. Adversarial scenarios use a helper that mutates state between
plan and execute steps.

```rust
#[tokio::test]
#[ignore]  // requires Docker
async fn push_injects_into_container_not_merge_to_target() {
    let session = TestSession::new("push-inject").await;
    let (repo_path, _cleanup) = colima_visible_repo("push-inject-repo");

    // Setup: container and session in sync, target ahead
    // ... seed container, add commits to main on host ...

    let engine = SyncEngine::new(session.docker.clone());
    let plan = engine.plan_sync(&name, "main", &repos).await.unwrap();

    // Verify plan: push should be Inject, not Skip
    let action = &plan.action.repo_actions[0];
    assert!(matches!(action.state.push_action(), PushAction::Inject { .. }));

    let target_before = read_head(&repo_path, "main");

    // Execute push
    let result = engine.execute_push(&name, plan.action, &repos).await.unwrap();

    let target_after = read_head(&repo_path, "main");

    // THE PROPERTY: target branch unchanged by push
    assert_eq!(target_before, target_after,
        "push must not modify target branch");

    // Container should have advanced
    let snap = engine.snapshot(&name, "main").await.unwrap();
    // ... verify container HEAD changed ...
}
```

## Generating the Test Matrix

The TLA+ state space gives us the combinations. We can generate the
test matrix mechanically:

1. Enumerate reachable (LegState, MergeLeg) pairs
2. For each pair: what PullAction and PushAction are derived?
3. For each derived action: what git setup produces this state?
4. For each action execution: what are the postconditions?
5. For each postcondition: what adversarial mutation could violate it?

The "what git setup produces this state" mapping is the key creative
step — it's the inverse of classify. For each LegState, there's a
recipe:

| LegState | Git setup recipe |
|---|---|
| InSync | container HEAD == session HEAD |
| ContainerAhead | add commits in container after clone |
| SessionAhead | add commits to session branch on host |
| Diverged | add different commits on both sides |
| Unknown | container has commit not fetched to host |
| NoSessionBranch | don't extract (no session branch on host) |
| NoContainer | repo on host but not in container |
| ContentIdentical | squash-merge then modify history |

| MergeLeg | Git setup recipe |
|---|---|
| NoTarget | don't specify target branch |
| InSync | session == target |
| SessionAhead | extract puts session ahead of target |
| TargetAhead | push commits to target after last sync |
| Diverged | both session and target have independent commits |
| ContentIdentical | squash then target has equivalent content |

## Acceptance Criteria

- Layer 1: ≥40 pure derivation tests covering all reachable state triples
- Layer 2: ≥11 merge safety tests (7 outcome paths + 4 adversarial)
- Layer 3: ≥8 pipeline tests (from table above)
- All tests assert the safety invariant: target never has conflict markers
- All Layer 1 tests run without Docker (cargo test, not --ignored)
- Zero test references to SyncDecision (uses new types only)
