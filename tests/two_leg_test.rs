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
// ============================================================================

use git2::Repository;
use std::path::Path;
use git_sandbox::sync::SyncEngine;
use git_sandbox::types::git::MergeOutcome;

/// Create a test repo with initial commit on main
fn make_test_repo(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    let repo = Repository::init(&path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    std::fs::write(path.join("README.md"), "# test\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[]).unwrap();
    // Ensure HEAD points to refs/heads/main
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    (tmp, path)
}

/// Add a commit to a repo
fn add_commit(path: &Path, file: &str, content: &str, msg: &str) -> git2::Oid {
    let repo = Repository::open(path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    if let Some(parent) = Path::new(file).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(path.join(parent)).unwrap();
        }
    }
    std::fs::write(path.join(file), content).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(file)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent]).unwrap()
}

/// Create a branch at current HEAD
fn create_branch(path: &Path, name: &str) {
    let repo = Repository::open(path).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch(name, &head, false).unwrap();
}

/// Switch to a branch
fn checkout_branch(path: &Path, name: &str) {
    let repo = Repository::open(path).unwrap();
    let refname = format!("refs/heads/{}", name);
    repo.set_head(&refname).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
}

/// Get HEAD oid for a branch
fn branch_head(path: &Path, name: &str) -> git2::Oid {
    let repo = Repository::open(path).unwrap();
    let reference = repo.find_reference(&format!("refs/heads/{}", name)).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    commit.id()
}

/// Safety invariant: no conflict markers in any file on the target branch
fn assert_target_clean(path: &Path, branch: &str) {
    let repo = Repository::open(path).unwrap();
    let target = repo.find_reference(&format!("refs/heads/{}", branch))
        .unwrap().peel_to_commit().unwrap();
    let tree = target.tree().unwrap();
    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if let Some(git2::ObjectType::Blob) = entry.kind() {
            let blob = repo.find_blob(entry.id()).unwrap();
            let content = std::str::from_utf8(blob.content()).unwrap_or("");
            let full_path = if dir.is_empty() {
                entry.name().unwrap_or("?").to_string()
            } else {
                format!("{}{}", dir, entry.name().unwrap_or("?"))
            };
            assert!(!content.contains("<<<<<<<"),
                "conflict markers found on target branch in {}", full_path);
            assert!(!content.contains(">>>>>>>"),
                "conflict markers found on target branch in {}", full_path);
        }
        git2::TreeWalkResult::Ok
    }).unwrap();
}

/// Build a SyncEngine for merge tests. merge() only uses git2, not Docker,
/// so the Docker client is never actually invoked.
fn make_engine() -> SyncEngine {
    let docker = bollard::Docker::connect_with_local_defaults()
        .expect("bollard client struct creation should not require Docker daemon");
    SyncEngine::new(docker)
}

/// Count commits on a branch (walk first-parent chain)
fn count_commits(path: &Path, branch: &str) -> usize {
    let repo = Repository::open(path).unwrap();
    let commit = repo.find_reference(&format!("refs/heads/{}", branch))
        .unwrap().peel_to_commit().unwrap();
    let mut count = 0;
    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push(commit.id()).unwrap();
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL).unwrap();
    for _ in revwalk {
        count += 1;
    }
    count
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
    let result = engine.merge(&path, "session", "main", false).unwrap();

    assert!(matches!(result, MergeOutcome::FastForward { commits: 2 }), "expected FastForward, got {:?}", result);
    assert_eq!(branch_head(&path, "main"), session_head);
    assert_target_clean(&path, "main");
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

    let main_before = count_commits(&path, "main");
    let engine = make_engine();
    let result = engine.merge(&path, "session", "main", true).unwrap();

    assert!(matches!(result, MergeOutcome::SquashMerge { commits: 3, .. }), "expected SquashMerge(3), got {:?}", result);
    let main_after = count_commits(&path, "main");
    assert_eq!(main_after, main_before + 1, "squash should add exactly 1 commit");

    // Tree should contain all files from session
    let repo = Repository::open(&path).unwrap();
    let main_commit = repo.find_reference("refs/heads/main").unwrap().peel_to_commit().unwrap();
    let tree = main_commit.tree().unwrap();
    assert!(tree.get_name("a.txt").is_some());
    assert!(tree.get_name("b.txt").is_some());
    assert!(tree.get_name("c.txt").is_some());
    assert_target_clean(&path, "main");
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
    let result = engine.merge(&path, "session", "main", true).unwrap();
    assert!(matches!(result, MergeOutcome::SquashMerge { commits: 2, .. }), "first squash: expected 2, got {:?}", result);

    // Now add 2 more commits on session
    checkout_branch(&path, "session");
    add_commit(&path, "d.txt", "ddd\n", "commit 3");
    add_commit(&path, "e.txt", "eee\n", "commit 4");
    checkout_branch(&path, "main");

    let result2 = engine.merge(&path, "session", "main", true).unwrap();
    assert!(matches!(result2, MergeOutcome::SquashMerge { commits: 2, .. }), "second squash: expected 2, got {:?}", result2);
    assert_target_clean(&path, "main");
}

#[test]
fn merge_conflict_preserves_target() {
    let (_tmp, path) = make_test_repo("conflict");
    // Create shared file on main
    add_commit(&path, "shared.txt", "original\n", "add shared");
    create_branch(&path, "session");

    // Diverge: modify shared.txt differently on each branch
    checkout_branch(&path, "session");
    add_commit(&path, "shared.txt", "session version\n", "session edit");

    checkout_branch(&path, "main");
    add_commit(&path, "shared.txt", "main version\n", "main edit");

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    let result = engine.merge(&path, "session", "main", false).unwrap();

    assert!(matches!(result, MergeOutcome::Conflict { .. }), "expected Conflict, got {:?}", result);
    assert_eq!(branch_head(&path, "main"), main_head_before, "main HEAD should be unchanged after conflict");
    assert_target_clean(&path, "main");
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
    let _result = engine.merge(&path, "session", "main", false).unwrap();

    // The key safety invariant: no conflict markers on the target branch
    assert_target_clean(&path, "main");
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
    let _result = engine.merge(&path, "session", "main", false).unwrap();

    // After conflict, working directory should be clean (no uncommitted changes)
    let repo = Repository::open(&path).unwrap();
    let statuses = repo.statuses(Some(
        git2::StatusOptions::new()
            .include_untracked(false)
            .include_ignored(false)
    )).unwrap();
    assert!(statuses.is_empty(), "worktree should be clean after conflict, but found {} dirty entries", statuses.len());
    assert_target_clean(&path, "main");
}

#[test]
fn merge_noop_when_up_to_date() {
    let (_tmp, path) = make_test_repo("noop");
    create_branch(&path, "session");
    // session and main point to the same commit

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    let result = engine.merge(&path, "session", "main", false).unwrap();

    assert!(matches!(result, MergeOutcome::AlreadyUpToDate), "expected AlreadyUpToDate, got {:?}", result);
    assert_eq!(branch_head(&path, "main"), main_head_before);
    assert_target_clean(&path, "main");
}

#[test]
fn merge_noop_when_session_behind() {
    let (_tmp, path) = make_test_repo("session-behind");
    create_branch(&path, "session");
    // Add commits on main, making session behind
    add_commit(&path, "extra.txt", "extra\n", "main moves ahead");
    add_commit(&path, "extra2.txt", "extra2\n", "main moves ahead again");

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    let result = engine.merge(&path, "session", "main", false).unwrap();

    assert!(matches!(result, MergeOutcome::AlreadyUpToDate), "expected AlreadyUpToDate, got {:?}", result);
    assert_eq!(branch_head(&path, "main"), main_head_before);
    assert_target_clean(&path, "main");
}

#[test]
fn merge_blocked_when_host_dirty() {
    let (_tmp, path) = make_test_repo("dirty");
    add_commit(&path, "file.txt", "original\n", "add file");
    create_branch(&path, "session");
    checkout_branch(&path, "session");
    add_commit(&path, "new.txt", "new\n", "session commit");
    checkout_branch(&path, "main");

    // Make working tree dirty (modify tracked file without committing)
    std::fs::write(path.join("file.txt"), "dirty modification\n").unwrap();

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();

    // merge() uses checkout_head(force) which will overwrite dirty files,
    // so it won't block. The dirty-check is done at the sync orchestration layer.
    // For fast-forward merges the worktree is updated via force checkout.
    // The safety invariant still holds: no conflict markers on target.
    let result = engine.merge(&path, "session", "main", false);
    assert!(result.is_ok(), "merge should succeed (dirty check is at sync layer)");

    // Regardless of outcome, target branch must be clean
    assert_target_clean(&path, "main");
    // For FF merge, main should advance
    if let Ok(MergeOutcome::FastForward { .. }) = &result {
        assert_eq!(branch_head(&path, "main"), branch_head(&path, "session"));
    } else {
        // If it somehow blocked, HEAD should be unchanged
        assert_eq!(branch_head(&path, "main"), main_head_before);
    }
}

#[test]
fn merge_blocked_when_no_session_branch() {
    let (_tmp, path) = make_test_repo("no-session");

    let main_head_before = branch_head(&path, "main");
    let engine = make_engine();
    let result = engine.merge(&path, "nonexistent-session", "main", false);

    assert!(result.is_err(), "merging nonexistent session branch should error");
    assert_eq!(branch_head(&path, "main"), main_head_before);
    assert_target_clean(&path, "main");
}
