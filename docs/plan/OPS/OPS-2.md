# OPS-2: Test Foundation — Pin User-Facing Behavior

blocked_by: [OPS-1.5]
unlocks: [OPS-3, OPS-4, OPS-5, VM-2]

## Why first

Everything after this changes behavior. We need tests that prove the
tool does what the user expects before we refactor internals.

## Test structure

Two layers:
- **Scenario tests**: real git repos, test the full plan→action path
  a user would see. Named for what the user did.
- **Safety invariant**: asserted in every test that touches the target
  branch — "my main was not corrupted."

Docker tests (inject/extract) are `#[ignore]` — they pin the full
pipeline. Non-Docker tests cover planning + merge (the dangerous part).

## Scenarios

### Push

| Test | User story | Setup | Assert |
|---|---|---|---|
| `push_delivers_commits_to_plan` | "I have commits on host, push shows them" | Host branch 3 ahead of container | Plan shows Inject{3}, not Skip |
| `push_after_squash_merge_still_injects` | "I squash-merged into main, push sees the new work" | Container==session, target diverged after squash | Plan shows Inject (not MergeToTarget, not unchanged) |
| `push_with_dirty_host_still_works` | "I have uncommitted changes, push shouldn't care" | Host dirty, target 2 ahead | Plan shows Inject{2}, not Blocked |
| `push_with_dirty_container_is_blocked` | "Container has uncommitted work, normal push blocked" | Container dirty, target ahead | Plan shows Blocked |
| `push_with_dirty_container_force_unblocks` | "Force push overrides dirty container" | Container dirty, target ahead | force_inject called, not blocked |
| `push_does_not_touch_target_branch` | "Push never modifies my main" | Target ahead, push plan | push_action never produces MergeToTarget |
| `push_idempotent_after_inject` | "Second push shows nothing to do" | After inject + re-extract | Plan shows Skip for all repos |

### Pull

| Test | User story | Setup | Assert |
|---|---|---|---|
| `pull_extracts_container_work` | "Container has new commits, pull gets them" | Container 5 ahead of session | Plan shows Extract{5} |
| `pull_merges_into_target` | "Session has work, pull merges into main" | Session ahead of target | Plan shows MergeToTarget |
| `pull_conflict_leaves_target_unchanged` | "Merge conflict doesn't corrupt my branch" | Conflicting file on both branches | merge() → Conflict, target_head unchanged |
| `pull_conflict_no_markers_committed` | "Conflict markers never end up on main" | Conflicting file | No `<<<<<<<` in target tree after merge |
| `pull_conflict_worktree_clean` | "After conflict, my worktree is clean" | Conflicting file | git status clean after conflict rollback |
| `pull_with_dirty_host_is_blocked` | "Uncommitted changes block pull (it writes to worktree)" | Host dirty, container ahead | Plan shows Blocked |

### Sync

| Test | User story | Setup | Assert |
|---|---|---|---|
| `sync_both_directions_detected` | "Container ahead AND target ahead" | Container +3, target +2 | Pull shows Extract, push shows Inject |
| `sync_identical_shows_no_work` | "Everything in sync, nothing to do" | All refs equal | has_work() == false |
| `sync_squash_identical_shows_no_work` | "After squash, content same but history differs" | Content identical, SHAs differ | has_work() == false |

### Merge safety (git2, no Docker — the target protection suite)

These create real git repos and call `merge()` directly. Every test
asserts the safety invariant.

| Test | Scenario | Assert |
|---|---|---|
| `merge_ff_advances_target` | Session 2 ahead, fast-forward possible | target_head = session_head, no markers |
| `merge_squash_creates_single_commit` | Session 3 ahead, squash mode | 1 new commit on target, tree matches session |
| `merge_squash_only_new_commits` | Squash-base exists, 2 new commits | Only 2 squashed, not full history |
| `merge_squash_stale_base_falls_back` | Session rebased, squash-base invalid | Uses merge-base, doesn't crash |
| `merge_conflict_preserves_target` | Both modified same file | target_head UNCHANGED, no markers |
| `merge_conflict_restores_worktree` | Both modified same file | Worktree matches target HEAD after |
| `merge_noop_when_up_to_date` | Session == target | AlreadyUpToDate, nothing changed |
| `merge_noop_when_behind` | Target ahead of session | AlreadyUpToDate |
| `merge_blocked_when_host_dirty` | Uncommitted changes in worktree | Err returned, target unchanged |
| `merge_blocked_when_no_session` | Session branch doesn't exist | Err returned, target unchanged |

**Safety invariant** (asserted in EVERY merge test):
```rust
fn assert_target_clean(repo_path: &Path, branch: &str) {
    let repo = Repository::open(repo_path).unwrap();
    let target = repo.find_reference(&format!("refs/heads/{}", branch))
        .unwrap().peel_to_commit().unwrap();
    let tree = target.tree().unwrap();
    tree.walk(git2::TreeWalkMode::PreOrder, |_, entry| {
        if let Some(git2::ObjectType::Blob) = entry.kind() {
            let blob = repo.find_blob(entry.id()).unwrap();
            let content = std::str::from_utf8(blob.content()).unwrap_or("");
            assert!(!content.contains("<<<<<<<"),
                "conflict markers on target branch in {}", entry.name().unwrap_or("?"));
        }
        git2::TreeWalkResult::Ok
    }).unwrap();
}
```

### Edge cases

| Test | What happened | Assert |
|---|---|---|
| `external_commits_on_target_detected` | Someone else pushed to main while agent worked | Plan shows target ahead, push offers inject |
| `container_matches_target_after_inject` | Push completed, container has main's content | ContentIdentical or InSync, no phantom work |
| `session_stale_after_force_push` | User force-pushed main | Plan handles gracefully (no crash, shows diverged) |

## File locations

- `tests/two_leg_test.rs` — all scenario + merge safety tests
- `src/types/git.rs` — add parallel two-leg assertions to existing sync_decision_tests

## Acceptance criteria

- ≥7 push scenario tests
- ≥6 pull scenario tests
- ≥3 sync scenario tests
- ≥10 merge safety tests, each asserting no conflict markers on target
- ≥3 edge case tests
- Squash-push regression test exists and passes
- Host-dirty-not-blocking-push test exists and passes
- All run without Docker (`cargo test`, no --ignored)
- sync_decision_tests have parallel two-leg assertions
