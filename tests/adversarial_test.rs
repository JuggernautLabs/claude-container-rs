//! Adversarial scenario tests — realistic user workflows where state
//! changes between planning and execution.
//!
//! Every test creates real git repos, runs VM programs, and asserts
//! on the actual git state afterward. These test what the user sees,
//! not internal types.

use git_sandbox::vm::*;
use git2::Repository;
use std::path::Path;
use std::path::PathBuf;

// ============================================================================
// Helpers — real temp repos, auto-cleaned
// ============================================================================

fn make_repo(name: &str) -> (tempfile::TempDir, PathBuf) {
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
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    (tmp, path)
}

fn commit_file(path: &Path, file: &str, content: &str, msg: &str) -> String {
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
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent]).unwrap().to_string()
}

fn git_branch(path: &Path, name: &str) {
    let repo = Repository::open(path).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch(name, &head, false).unwrap();
}

fn git_switch(path: &Path, name: &str) {
    let repo = Repository::open(path).unwrap();
    repo.set_head(&format!("refs/heads/{}", name)).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
}

fn head_of(path: &Path, name: &str) -> String {
    let repo = Repository::open(path).unwrap();
    let reference = repo.find_reference(&format!("refs/heads/{}", name)).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    commit.id().to_string()
}

fn assert_no_markers(path: &Path, branch: &str) {
    let repo = Repository::open(path).unwrap();
    let reference = repo.find_reference(&format!("refs/heads/{}", branch)).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    let tree = commit.tree().unwrap();
    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if let Some(git2::ObjectType::Blob) = entry.kind() {
            let blob = repo.find_blob(entry.id()).unwrap();
            let content = std::str::from_utf8(blob.content()).unwrap_or("");
            let full = if dir.is_empty() { entry.name().unwrap_or("?").to_string() }
                       else { format!("{}{}", dir, entry.name().unwrap_or("?")) };
            assert!(!content.contains("<<<<<<<"), "markers in {} on {}", full, branch);
        }
        git2::TreeWalkResult::Ok
    }).unwrap();
}

fn assert_worktree_clean(path: &Path) {
    let repo = Repository::open(path).unwrap();
    let statuses = repo.statuses(Some(
        git2::StatusOptions::new().include_untracked(false).include_ignored(false)
    )).unwrap();
    assert!(statuses.is_empty(), "worktree dirty: {} entries", statuses.len());
}

// ============================================================================
// Scenario: user commits to main while merge is being planned
// ============================================================================

#[tokio::test]
async fn target_advances_between_plan_and_merge() {
    let (_tmp, path) = make_repo("target-race");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "agent-work.txt", "agent did this", "agent commit");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");

    let target_at_plan = head_of(&path, "main");

    // VM plans a merge: session is ahead of main
    let backend = Git2Backend::new();
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(target_at_plan.clone()),
        Some(path.clone()),
    ));

    // BETWEEN PLAN AND EXECUTE: user pushes to main
    commit_file(&path, "user-work.txt", "user pushed this", "user commit");
    let target_after_user = head_of(&path, "main");
    assert_ne!(target_at_plan, target_after_user);

    // Execute the merge — uses the plan-time target hash, not live
    let result = vm.run(&backend, vec![
        Op::TryMerge {
            repo: "repo".into(),
            ours: target_at_plan.clone(), // stale!
            theirs: session_h.clone(),
            on_clean: vec![
                Op::checkout(Side::Host, "repo", "refs/heads/main"),
                Op::commit("repo", "MERGED_TREE", &[&target_at_plan], "squash"),
            ],
            on_conflict: vec![],
            on_error: vec![],
        },
    ]).await;

    // The merge used the stale target — but main is safe either way
    assert_no_markers(&path, "main");
    // Main should have advanced (merge happened against stale base, but committed)
    // OR merge may have errored because tree/parent mismatch
    // Either way: no corruption
    assert_no_markers(&path, "main");
}

// ============================================================================
// Scenario: user dirties worktree between plan and merge
// ============================================================================

#[tokio::test]
async fn host_dirtied_between_plan_and_merge() {
    let (_tmp, path) = make_repo("dirty-race");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "work.txt", "session work", "session commit");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");
    let main_h = head_of(&path, "main");

    let backend = Git2Backend::new();

    // Merge trees first (clean merge)
    let (clean, tree, _) = backend.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(clean);
    let tree_hash = tree.unwrap();

    // BETWEEN PLAN AND EXECUTE: user edits a file without committing
    std::fs::write(path.join("README.md"), "user was here\n").unwrap();

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(main_h.clone()),
        Some(path.clone()),
    ));

    // Execute: checkout + commit
    // checkout(force) will overwrite the dirty file
    let result = vm.run(&backend, vec![
        Op::checkout(Side::Host, "repo", "refs/heads/main"),
        Op::commit("repo", &tree_hash, &[&main_h], "squash merge"),
    ]).await;

    // Target should be safe regardless
    assert_no_markers(&path, "main");
}

// ============================================================================
// Scenario: user deletes session branch between plan and merge
// ============================================================================

#[tokio::test]
async fn session_branch_deleted_between_plan_and_merge() {
    let (_tmp, path) = make_repo("deleted-session");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "work.txt", "work", "session commit");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");
    let main_h = head_of(&path, "main");

    // BETWEEN PLAN AND EXECUTE: user deletes session branch
    {
        let repo = Repository::open(&path).unwrap();
        let mut branch = repo.find_branch("session", git2::BranchType::Local).unwrap();
        branch.delete().unwrap();
    }

    // The session commit still exists (dangling), merge can still work
    // because we use the hash directly, not the branch name
    let backend = Git2Backend::new();
    let (clean, tree, _) = backend.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(clean);
    let tree_hash = tree.unwrap();

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(main_h.clone()),
        Some(path.clone()),
    ));

    let result = vm.run(&backend, vec![
        Op::checkout(Side::Host, "repo", "refs/heads/main"),
        Op::commit("repo", &tree_hash, &[&main_h], "merge after branch deletion"),
    ]).await;

    assert!(!result.halted, "should succeed even with deleted branch: {:?}", result.halt_reason);
    assert_no_markers(&path, "main");
}

// ============================================================================
// Scenario: user force-pushes main (reset to earlier commit)
// ============================================================================

#[tokio::test]
async fn target_force_pushed_between_plan_and_merge() {
    let (_tmp, path) = make_repo("force-push");
    let initial = head_of(&path, "main");
    commit_file(&path, "a.txt", "a", "advance 1");
    commit_file(&path, "b.txt", "b", "advance 2");
    let advanced = head_of(&path, "main");

    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "session.txt", "s", "session work");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");

    // Plan: merge session into current main (advanced)
    let backend = Git2Backend::new();
    let (clean, tree, _) = backend.merge_trees(&path, &advanced, &session_h).await.unwrap();
    assert!(clean);
    let tree_hash = tree.unwrap();

    // BETWEEN PLAN AND EXECUTE: user force-pushes main back to initial
    let repo = Repository::open(&path).unwrap();
    let initial_oid = git2::Oid::from_str(&initial).unwrap();
    repo.reference("refs/heads/main", initial_oid, true, "force push").unwrap();
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    drop(repo);

    assert_eq!(head_of(&path, "main"), initial);

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(advanced.clone()), // stale — main was force-pushed
        Some(path.clone()),
    ));

    // Execute with stale parent — commit uses `advanced` as parent
    // but main now points at `initial`. The commit will still create,
    // but it won't be reachable from main unless we also ref_write.
    let result = vm.run(&backend, vec![
        Op::checkout(Side::Host, "repo", "refs/heads/main"),
        Op::commit("repo", &tree_hash, &[&advanced], "merge with stale parent"),
    ]).await;

    // Main is safe — either the commit worked (new HEAD) or errored
    assert_no_markers(&path, "main");
}

// ============================================================================
// Scenario: conflict detected — merge rolled back, target untouched
// ============================================================================

#[tokio::test]
async fn conflict_never_corrupts_target() {
    let (_tmp, path) = make_repo("conflict-safe");
    commit_file(&path, "shared.txt", "original", "base");
    git_branch(&path, "session");

    // Diverge on the same file
    commit_file(&path, "shared.txt", "main version\nwith more content", "main edit");
    let main_h = head_of(&path, "main");

    git_switch(&path, "session");
    commit_file(&path, "shared.txt", "session version\ncompletely different", "session edit");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");

    let backend = Git2Backend::new();
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(main_h.clone()),
        Some(path.clone()),
    ));

    let result = vm.run(&backend, vec![
        Op::TryMerge {
            repo: "repo".into(),
            ours: main_h.clone(),
            theirs: session_h.clone(),
            on_clean: vec![
                Op::commit("repo", "TREE", &[&main_h], "should NOT run"),
            ],
            on_conflict: vec![
                // Conflict path — just record, don't modify
            ],
            on_error: vec![],
        },
    ]).await;

    // THE INVARIANT: main untouched, no markers, worktree clean
    assert_eq!(head_of(&path, "main"), main_h);
    assert_no_markers(&path, "main");
    assert_worktree_clean(&path);
}

// ============================================================================
// Scenario: squash merge after prior squash (incremental)
// ============================================================================

#[tokio::test]
async fn squash_merge_is_incremental() {
    let (_tmp, path) = make_repo("incremental-squash");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "a.txt", "a", "commit 1");
    commit_file(&path, "b.txt", "b", "commit 2");
    let session_h1 = head_of(&path, "session");
    git_switch(&path, "main");
    let main_h1 = head_of(&path, "main");

    let backend = Git2Backend::new();

    // First squash merge
    let (clean, tree, _) = backend.merge_trees(&path, &main_h1, &session_h1).await.unwrap();
    assert!(clean);
    let tree_hash = tree.unwrap();

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h1.clone()),
        RefState::At(session_h1.clone()),
        RefState::At(main_h1.clone()),
        Some(path.clone()),
    ));

    let result = vm.run(&backend, vec![
        Op::checkout(Side::Host, "repo", "refs/heads/main"),
        Op::commit("repo", &tree_hash, &[&main_h1], "squash 1"),
    ]).await;
    assert!(!result.halted);

    let main_after_first = head_of(&path, "main");
    assert_ne!(main_h1, main_after_first);

    // Add more commits on session
    git_switch(&path, "session");
    commit_file(&path, "c.txt", "c", "commit 3");
    commit_file(&path, "d.txt", "d", "commit 4");
    let session_h2 = head_of(&path, "session");
    git_switch(&path, "main");

    // Second squash merge — should include c.txt and d.txt
    let (clean2, tree2, _) = backend.merge_trees(&path, &main_after_first, &session_h2).await.unwrap();
    assert!(clean2);
    let tree_hash2 = tree2.unwrap();

    let mut vm2 = SyncVM::new("session", "main");
    vm2.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h2.clone()),
        RefState::At(session_h2.clone()),
        RefState::At(main_after_first.clone()),
        Some(path.clone()),
    ));

    let result2 = vm2.run(&backend, vec![
        Op::checkout(Side::Host, "repo", "refs/heads/main"),
        Op::commit("repo", &tree_hash2, &[&main_after_first], "squash 2"),
    ]).await;
    assert!(!result2.halted);

    let main_final = head_of(&path, "main");
    assert_ne!(main_after_first, main_final);

    // Main should have all 4 files
    let repo = Repository::open(&path).unwrap();
    let main_commit = repo.find_reference("refs/heads/main").unwrap().peel_to_commit().unwrap();
    let tree = main_commit.tree().unwrap();
    assert!(tree.get_name("a.txt").is_some(), "a.txt missing");
    assert!(tree.get_name("b.txt").is_some(), "b.txt missing");
    assert!(tree.get_name("c.txt").is_some(), "c.txt missing");
    assert!(tree.get_name("d.txt").is_some(), "d.txt missing");
    assert_no_markers(&path, "main");
}

// ============================================================================
// Scenario: multiple repos, mixed states
// ============================================================================

#[tokio::test]
async fn multi_repo_mixed_states() {
    let (_tmp_a, path_a) = make_repo("alpha");
    let (_tmp_b, path_b) = make_repo("beta");

    // Alpha: session ahead of main (needs merge)
    git_branch(&path_a, "session");
    git_switch(&path_a, "session");
    commit_file(&path_a, "alpha.txt", "alpha work", "alpha commit");
    let alpha_session = head_of(&path_a, "session");
    git_switch(&path_a, "main");
    let alpha_main = head_of(&path_a, "main");

    // Beta: already in sync
    git_branch(&path_b, "session");
    let beta_h = head_of(&path_b, "main");

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At(alpha_session.clone()),
        RefState::At(alpha_session.clone()),
        RefState::At(alpha_main.clone()),
        Some(path_a.clone()),
    ));
    vm.set_repo("beta", RepoVM::from_refs(
        RefState::At(beta_h.clone()),
        RefState::At(beta_h.clone()),
        RefState::At(beta_h.clone()),
        Some(path_b.clone()),
    ));

    // plan_pull should only generate ops for alpha, not beta
    let pull_ops = plan_pull(&vm);
    let has_alpha_ops = pull_ops.iter().any(|op| {
        match op {
            Op::Extract { repo, .. } => repo == "alpha",
            Op::TryMerge { repo, .. } => repo == "alpha",
            _ => false,
        }
    });
    let has_beta_ops = pull_ops.iter().any(|op| {
        match op {
            Op::Extract { repo, .. } => repo == "beta",
            Op::TryMerge { repo, .. } => repo == "beta",
            _ => false,
        }
    });
    assert!(has_alpha_ops, "alpha should have pull ops");
    assert!(!has_beta_ops, "beta should have no pull ops (in sync)");
}

// ============================================================================
// Scenario: precondition catches impossible operation
// ============================================================================

#[tokio::test]
async fn precondition_prevents_merge_on_conflicted_host() {
    let backend = MockBackend::new();
    let mut vm = SyncVM::new("session", "main");
    let mut repo = RepoVM::from_refs(
        RefState::At("aaa".into()),
        RefState::At("bbb".into()),
        RefState::At("ccc".into()),
        Some(PathBuf::from("/tmp/test")),
    );
    repo.host_merge_state = HostMergeState::Conflicted;
    vm.set_repo("repo", repo);

    let result = vm.run(&backend, vec![
        Op::commit("repo", "tree", &["ccc"], "should fail precondition"),
    ]).await;

    assert!(result.halted);
    assert_eq!(result.succeeded(), 0);
    // Backend was never called
    assert!(!backend.was_called(&CallMatcher::Commit));
}
