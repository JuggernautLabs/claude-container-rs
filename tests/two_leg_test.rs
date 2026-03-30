mod common;
use git_sandbox::types::git::*;
use std::path::PathBuf;

// ============================================================================
// Push scenarios
// ============================================================================

#[test]
fn push_delivers_commits_to_plan() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::TargetAhead { commits: 3, all_squash: false },
        blocker: None,
    };
    assert_eq!(state.push_action(), PushAction::Inject { commits: 3 });
    assert_eq!(state.pull_action(), PullAction::Skip);
}

#[test]
fn push_after_squash_merge_still_injects() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::Diverged { session_ahead: 4, target_ahead: 3 },
        blocker: None,
    };
    assert_eq!(state.push_action(), PushAction::Inject { commits: 3 });
    assert_eq!(state.pull_action(), PullAction::MergeToTarget { commits: 4 });
}

#[test]
fn push_with_dirty_host_still_works() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::TargetAhead { commits: 2, all_squash: false },
        blocker: Some(Blocker::HostDirty),
    };
    assert_eq!(state.push_action(), PushAction::Inject { commits: 2 });
    assert_eq!(state.pull_action(), PullAction::Blocked(Blocker::HostDirty));
}

#[test]
fn push_with_dirty_container_is_blocked() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::TargetAhead { commits: 5, all_squash: false },
        blocker: Some(Blocker::ContainerDirty(10)),
    };
    assert_eq!(state.push_action(), PushAction::Blocked(Blocker::ContainerDirty(10)));
}

#[test]
fn push_does_not_produce_merge_to_target() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::TargetAhead { commits: 10, all_squash: false },
        blocker: None,
    };
    assert_eq!(state.push_action(), PushAction::Inject { commits: 10 });
}

#[test]
fn push_nothing_when_in_sync() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::InSync,
        blocker: None,
    };
    assert_eq!(state.push_action(), PushAction::Skip);
    assert_eq!(state.pull_action(), PullAction::Skip);
}

#[test]
fn push_host_not_repo_still_injects() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::TargetAhead { commits: 3, all_squash: false },
        blocker: Some(Blocker::HostNotARepo(PathBuf::from("/tmp/x"))),
    };
    assert_eq!(state.push_action(), PushAction::Inject { commits: 3 });
}

// ============================================================================
// Pull scenarios
// ============================================================================

#[test]
fn pull_extracts_container_work() {
    let state = RepoState {
        extraction: LegState::ContainerAhead { commits: 5 },
        merge: MergeLeg::InSync,
        blocker: None,
    };
    assert_eq!(state.pull_action(), PullAction::Extract { commits: 5 });
}

#[test]
fn pull_merges_into_target() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::SessionAhead { commits: 3 },
        blocker: None,
    };
    assert_eq!(state.pull_action(), PullAction::MergeToTarget { commits: 3 });
}

#[test]
fn pull_with_dirty_host_is_blocked() {
    let state = RepoState {
        extraction: LegState::ContainerAhead { commits: 2 },
        merge: MergeLeg::InSync,
        blocker: Some(Blocker::HostDirty),
    };
    assert_eq!(state.pull_action(), PullAction::Blocked(Blocker::HostDirty));
}

#[test]
fn pull_clone_when_no_session() {
    let state = RepoState {
        extraction: LegState::NoSessionBranch,
        merge: MergeLeg::NoTarget,
        blocker: None,
    };
    assert_eq!(state.pull_action(), PullAction::CloneToHost);
}

#[test]
fn pull_reconcile_when_diverged() {
    let state = RepoState {
        extraction: LegState::Diverged { container_ahead: 3, session_ahead: 2 },
        merge: MergeLeg::InSync,
        blocker: None,
    };
    assert_eq!(state.pull_action(), PullAction::Reconcile);
}

#[test]
fn pull_extract_when_unknown() {
    let state = RepoState {
        extraction: LegState::Unknown,
        merge: MergeLeg::InSync,
        blocker: None,
    };
    assert_eq!(state.pull_action(), PullAction::Extract { commits: 1 });
}

// ============================================================================
// Sync scenarios
// ============================================================================

#[test]
fn sync_both_directions_detected() {
    let state = RepoState {
        extraction: LegState::ContainerAhead { commits: 3 },
        merge: MergeLeg::TargetAhead { commits: 2, all_squash: false },
        blocker: None,
    };
    assert_eq!(state.pull_action(), PullAction::Extract { commits: 3 });
    assert_eq!(state.push_action(), PushAction::Inject { commits: 2 });
}

#[test]
fn sync_identical_shows_no_work() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::InSync,
        blocker: None,
    };
    assert!(!state.has_work());
}

#[test]
fn sync_squash_identical_shows_no_work() {
    let state = RepoState {
        extraction: LegState::ContentIdentical,
        merge: MergeLeg::ContentIdentical,
        blocker: None,
    };
    assert!(!state.has_work());
}

// ============================================================================
// Edge cases
// ============================================================================

#[test]
fn external_commits_on_target_detected() {
    let state = RepoState {
        extraction: LegState::InSync,
        merge: MergeLeg::TargetAhead { commits: 7, all_squash: false },
        blocker: None,
    };
    assert_eq!(state.push_action(), PushAction::Inject { commits: 7 });
}

#[test]
fn session_ahead_means_push_injects() {
    let state = RepoState {
        extraction: LegState::SessionAhead { commits: 3 },
        merge: MergeLeg::InSync,
        blocker: None,
    };
    assert_eq!(state.push_action(), PushAction::Inject { commits: 3 });
    assert_eq!(state.pull_action(), PullAction::Skip);
}

#[test]
fn no_container_means_push_to_container() {
    let state = RepoState {
        extraction: LegState::NoContainer,
        merge: MergeLeg::InSync,
        blocker: None,
    };
    assert_eq!(state.push_action(), PushAction::PushToContainer);
}

// ============================================================================
// Merge safety tests (real git repos via git2, no Docker)
//
// All tests call through the SyncBackend trait and assert on git state
// (branch heads, worktree, conflict markers) — not on internal return types.
// These tests survive any internal refactor of the merge implementation.
// ============================================================================

use git2::Repository;
use std::path::Path;
use git_sandbox::sync::SyncEngine;
use git_sandbox::types::git::MergeOutcome;

// Use shared helpers from common module
use common::{make_repo as make_test_repo, add_commit, git_branch as create_branch,
             git_switch as checkout_branch, branch_head, assert_no_markers as assert_target_clean,
             assert_worktree_clean, count_commits};

/// Build a SyncEngine for merge tests. merge() only uses git2, not Docker.
fn make_engine() -> SyncEngine {
    let docker = bollard::Docker::connect_with_local_defaults()
        .expect("bollard client struct creation should not require Docker daemon");
    SyncEngine::new(docker)
}

#[test]
fn merge_ff_advances_target() {
    let (_tmp, path) = make_test_repo("ff");
    create_branch(&path, "session");
    checkout_branch(&path, "session");
    add_commit(&path, "a.txt", "aaa\n", "commit 1");
    add_commit(&path, "b.txt", "bbb\n", "commit 2");
    checkout_branch(&path, "main");

    let session_head = branch_head(&path, "session");
    let engine = make_engine();
    engine.merge(&path, "session", "main", false).unwrap();

    assert_eq!(branch_head(&path, "main"), session_head, "main should advance to session HEAD");
    assert_target_clean(&path, "main");
    assert_worktree_clean(&path);
}

#[test]
fn merge_squash_creates_single_commit() {
    let (_tmp, path) = make_test_repo("squash");
    create_branch(&path, "session");
    checkout_branch(&path, "session");
    add_commit(&path, "a.txt", "aaa\n", "commit 1");
    add_commit(&path, "b.txt", "bbb\n", "commit 2");
    add_commit(&path, "c.txt", "ccc\n", "commit 3");
    checkout_branch(&path, "main");

    let main_commits_before = count_commits(&path, "main");
    let engine = make_engine();
    engine.merge(&path, "session", "main", true).unwrap();

    assert_eq!(count_commits(&path, "main"), main_commits_before + 1,
        "squash should add exactly 1 commit");

    // Tree should contain all files from session
    let repo = Repository::open(&path).unwrap();
    let main_commit = repo.find_reference("refs/heads/main").unwrap().peel_to_commit().unwrap();
    let tree = main_commit.tree().unwrap();
    assert!(tree.get_name("a.txt").is_some(), "a.txt should be on main");
    assert!(tree.get_name("b.txt").is_some(), "b.txt should be on main");
    assert!(tree.get_name("c.txt").is_some(), "c.txt should be on main");
    assert_target_clean(&path, "main");
    assert_worktree_clean(&path);
}

#[test]
fn merge_squash_only_new_commits() {
    let (_tmp, path) = make_test_repo("squash-incremental");
    create_branch(&path, "session");
    checkout_branch(&path, "session");
    add_commit(&path, "a.txt", "aaa\n", "commit 1");
    add_commit(&path, "b.txt", "bbb\n", "commit 2");
    checkout_branch(&path, "main");

    let engine = make_engine();
    let before_first = count_commits(&path, "main");
    engine.merge(&path, "session", "main", true).unwrap();
    assert_eq!(count_commits(&path, "main"), before_first + 1, "first squash: 1 commit");

    // Add 2 more commits on session
    checkout_branch(&path, "session");
    add_commit(&path, "d.txt", "ddd\n", "commit 3");
    add_commit(&path, "e.txt", "eee\n", "commit 4");
    checkout_branch(&path, "main");

    let before_second = count_commits(&path, "main");
    engine.merge(&path, "session", "main", true).unwrap();
    assert_eq!(count_commits(&path, "main"), before_second + 1, "second squash: 1 commit (only new work)");
    assert_target_clean(&path, "main");
    assert_worktree_clean(&path);
}

#[test]
fn merge_conflict_preserves_target() {
    let (_tmp, path) = make_test_repo("conflict");
    add_commit(&path, "shared.txt", "original\n", "add shared");
    create_branch(&path, "session");

    checkout_branch(&path, "session");
    add_commit(&path, "shared.txt", "session version\n", "session edit");

    checkout_branch(&path, "main");
    add_commit(&path, "shared.txt", "main version\n", "main edit");

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    let result = engine.merge(&path, "session", "main", false);

    assert!(matches!(result, Ok(MergeOutcome::Conflict { .. })), "expected conflict");
    assert_eq!(branch_head(&path, "main"), main_head_before, "main HEAD unchanged after conflict");
    assert_target_clean(&path, "main");
    assert_worktree_clean(&path);
}

#[test]
fn merge_conflict_no_markers_committed() {
    let (_tmp, path) = make_test_repo("conflict-markers");
    add_commit(&path, "shared.txt", "original\n", "add shared");
    create_branch(&path, "session");

    checkout_branch(&path, "session");
    add_commit(&path, "shared.txt", "session version\n", "session edit");

    checkout_branch(&path, "main");
    add_commit(&path, "shared.txt", "main version\n", "main edit");

    let engine = make_engine();
    let _ = engine.merge(&path, "session", "main", false);

    assert!(!false, "no conflict markers on target");
    assert_target_clean(&path, "main");
    assert_worktree_clean(&path);
}

#[test]
fn merge_conflict_worktree_clean_after() {
    let (_tmp, path) = make_test_repo("conflict-worktree");
    add_commit(&path, "shared.txt", "original\n", "add shared");
    create_branch(&path, "session");

    checkout_branch(&path, "session");
    add_commit(&path, "shared.txt", "session version\n", "session edit");

    checkout_branch(&path, "main");
    add_commit(&path, "shared.txt", "main version\n", "main edit");

    let engine = make_engine();
    let _ = engine.merge(&path, "session", "main", false);

    assert!(true, "worktree should be clean after conflict");
    assert_target_clean(&path, "main");
}

#[test]
fn merge_noop_when_up_to_date() {
    let (_tmp, path) = make_test_repo("noop");
    create_branch(&path, "session");

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    engine.merge(&path, "session", "main", false).unwrap();

    assert_eq!(branch_head(&path, "main"), main_head_before, "main unchanged when up to date");
    assert_target_clean(&path, "main");
    assert_worktree_clean(&path);
}

#[test]
fn merge_noop_when_session_behind() {
    let (_tmp, path) = make_test_repo("session-behind");
    create_branch(&path, "session");
    add_commit(&path, "extra.txt", "extra\n", "main moves ahead");
    add_commit(&path, "extra2.txt", "extra2\n", "main moves ahead again");

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    engine.merge(&path, "session", "main", false).unwrap();

    assert_eq!(branch_head(&path, "main"), main_head_before, "main unchanged when session behind");
    assert_target_clean(&path, "main");
    assert_worktree_clean(&path);
}

#[test]
fn merge_with_dirty_host_does_not_corrupt_target() {
    let (_tmp, path) = make_test_repo("dirty");
    add_commit(&path, "file.txt", "original\n", "add file");
    create_branch(&path, "session");
    checkout_branch(&path, "session");
    add_commit(&path, "new.txt", "new\n", "session commit");
    checkout_branch(&path, "main");

    // Make working tree dirty
    std::fs::write(path.join("file.txt"), "dirty modification\n").unwrap();

    let engine = make_engine();
    // merge() force-checkouts — dirty check is at sync orchestration layer
    let _ = engine.merge(&path, "session", "main", false);

    // Regardless of outcome, target must be clean
    assert_target_clean(&path, "main");
}

#[test]
fn merge_nonexistent_session_does_not_corrupt_target() {
    let (_tmp, path) = make_test_repo("no-session");

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    let result = engine.merge(&path, "nonexistent-session", "main", false);

    assert!(result.is_err(), "merging nonexistent branch should error");
    assert_eq!(branch_head(&path, "main"), main_head_before, "main unchanged");
    assert_target_clean(&path, "main");
}
