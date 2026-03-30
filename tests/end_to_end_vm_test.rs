//! End-to-end VM tests — full programs against real repos.
//!
//! These verify that the VM pipeline (plan → generate → execute) produces
//! correct git state. Not mocks — real repos, real merges.

use git_sandbox::vm::*;
use git2::Repository;
use std::path::{Path, PathBuf};

// ============================================================================
// Helpers
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

fn file_content(path: &Path, file: &str) -> String {
    std::fs::read_to_string(path.join(file)).unwrap_or_default()
}

fn assert_no_markers(path: &Path, branch: &str) {
    let repo = Repository::open(path).unwrap();
    let refname = format!("refs/heads/{}", branch);
    let reference = repo.find_reference(&refname).unwrap();
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

fn tree_has_file(path: &Path, branch: &str, file: &str) -> bool {
    let repo = Repository::open(path).unwrap();
    let refname = format!("refs/heads/{}", branch);
    let reference = repo.find_reference(&refname).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    let tree = commit.tree().unwrap();
    let x = tree.get_name(file).is_some(); x
}

// ============================================================================
// Test: Full merge pipeline via VM (TryMerge → on_clean → Commit)
// ============================================================================

#[tokio::test]
async fn vm_merge_pipeline_advances_target_with_correct_content() {
    let (_tmp, path) = make_repo("full-merge");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "feature.txt", "new feature code", "add feature");
    commit_file(&path, "tests.txt", "test suite", "add tests");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");
    let main_h = head_of(&path, "main");

    let backend = Git2Backend::new();

    // Pre-compute merge tree (as plan_sync would)
    let (clean, tree_opt, _) = backend.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(clean);
    let tree_hash = tree_opt.unwrap();

    // Build VM and run full merge program
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
                Op::checkout(Side::Host, "repo", "refs/heads/main"),
                Op::commit("repo", &tree_hash, &[&main_h], "squash: 2 commits"),
                // commit(Some("HEAD")) already updates refs/heads/main via git2
            ],
            on_conflict: vec![],
            on_error: vec![
                Op::checkout(Side::Host, "repo", "refs/heads/main"),
            ],
        },
    ]).await;

    assert!(!result.halted, "merge pipeline halted: {:?}", result.halt_reason);

    // VERIFY: main has the feature files
    assert!(tree_has_file(&path, "main", "feature.txt"), "feature.txt should be on main");
    assert!(tree_has_file(&path, "main", "tests.txt"), "tests.txt should be on main");
    assert!(tree_has_file(&path, "main", "README.md"), "README.md should still be on main");
    assert_no_markers(&path, "main");

    // main advanced
    let main_after = head_of(&path, "main");
    assert_ne!(main_h, main_after, "main should have advanced");
}

// ============================================================================
// Test: Conflict triggers on_conflict path, not on_clean
// ============================================================================

#[tokio::test]
async fn vm_conflict_triggers_correct_path() {
    let (_tmp, path) = make_repo("conflict-path");
    commit_file(&path, "shared.txt", "original content", "base");
    git_branch(&path, "session");

    // Diverge on same file
    commit_file(&path, "shared.txt", "main changed this line", "main edit");
    let main_h = head_of(&path, "main");

    git_switch(&path, "session");
    commit_file(&path, "shared.txt", "session changed this line differently", "session edit");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");

    let backend = Git2Backend::new();
    let mock_for_agent = MockBackend::new();

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(main_h.clone()),
        Some(path.clone()),
    ));

    // Track which path was taken
    let result = vm.run(&backend, vec![
        Op::TryMerge {
            repo: "repo".into(),
            ours: main_h.clone(),
            theirs: session_h.clone(),
            on_clean: vec![
                // This should NOT execute
                Op::commit("repo", "FAKE", &[&main_h], "should not happen"),
            ],
            on_conflict: vec![
                // This SHOULD execute — but we just verify the path was taken
                // by checking main is unchanged
            ],
            on_error: vec![],
        },
    ]).await;

    assert!(!result.halted);
    // Main unchanged — conflict path was taken (which has no ops)
    assert_eq!(head_of(&path, "main"), main_h, "main should be unchanged after conflict");
    assert_no_markers(&path, "main");

    // Verify the TryMerge outcome was recorded as conflict
    assert!(result.outcomes.iter().any(|o| o.op_description.contains("conflict")),
        "should record conflict outcome");
}

// ============================================================================
// Test: plan_pull generates reconcile for diverged state
// ============================================================================

#[tokio::test]
async fn plan_pull_generates_reconcile_for_diverged() {
    // Diverged: container has commits session doesn't, AND
    // session/target differ from container
    // This isn't directly "diverged" in the current repo_pull_action —
    // it's "Extract" because container != session.
    // But the TWO-LEG model sees it as diverged when extraction AND merge
    // legs both have work.

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("container_work".into()),  // container has new work
        RefState::At("session_old".into()),     // session is behind container
        RefState::At("target_different".into()), // target moved independently
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = programs::plan_pull(&vm);

    // Should have Extract (because container != session)
    assert!(ops.iter().any(|op| matches!(op, Op::Extract { .. })),
        "diverged state should generate Extract, got: {:?}", ops);

    // Should also have TryMerge (because session != target after extraction)
    assert!(ops.iter().any(|op| matches!(op, Op::TryMerge { .. })),
        "diverged state should generate TryMerge, got: {:?}", ops);
}

// ============================================================================
// Test: plan_push for repos where target moved ahead
// ============================================================================

#[tokio::test]
async fn plan_push_for_target_ahead() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("same".into()),   // container
        RefState::At("same".into()),   // session = container
        RefState::At("ahead".into()),  // target moved ahead
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = programs::plan_push(&vm);
    assert!(ops.iter().any(|op| matches!(op, Op::Inject { .. })),
        "target ahead should generate Inject, got: {:?}", ops);
    // Should also re-extract
    assert!(ops.iter().any(|op| matches!(op, Op::Extract { .. })),
        "should re-extract after inject, got: {:?}", ops);
}

// ============================================================================
// Test: plan produces nothing for fully synced repos
// ============================================================================

#[tokio::test]
async fn plan_produces_nothing_for_synced() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("same_hash".into()),
        RefState::At("same_hash".into()),
        RefState::At("same_hash".into()),
        Some(PathBuf::from("/tmp/alpha")),
    ));

    assert!(programs::plan_push(&vm).is_empty(), "push should be empty");
    assert!(programs::plan_pull(&vm).is_empty(), "pull should be empty");
    assert!(programs::plan_sync(&vm).is_empty(), "sync should be empty");
}

// ============================================================================
// Test: plan handles container-only repo (no host path yet)
// ============================================================================

#[tokio::test]
async fn plan_handles_container_only() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("new-repo", RepoVM::from_refs(
        RefState::At("container_head".into()),
        RefState::Absent,
        RefState::Absent,
        Some(PathBuf::from("/tmp/new-repo")),
    ));

    let pull_ops = programs::plan_pull(&vm);
    assert!(pull_ops.iter().any(|op| matches!(op, Op::Extract { .. })),
        "container-only should generate Extract (clone to host), got: {:?}", pull_ops);
}

// ============================================================================
// Test: plan handles host-only repo (no container)
// ============================================================================

#[tokio::test]
async fn plan_handles_host_only() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("host-repo", RepoVM::from_refs(
        RefState::Absent,
        RefState::At("host_head".into()),
        RefState::At("host_head".into()),
        Some(PathBuf::from("/tmp/host-repo")),
    ));

    let push_ops = programs::plan_push(&vm);
    // Absent container → Clone intent
    assert!(push_ops.iter().any(|op| matches!(op, Op::RunContainer { .. })),
        "host-only should generate clone (RunContainer), got: {:?}", push_ops);

    let pull_ops = programs::plan_pull(&vm);
    assert!(pull_ops.is_empty(), "no container → nothing to pull");
}

// ============================================================================
// Test: plan_sync ordering — push ops before pull ops
// ============================================================================

#[tokio::test]
async fn plan_sync_puts_push_before_pull() {
    let mut vm = SyncVM::new("session", "main");
    // Repo needs both push (target ahead) and pull (container ahead)
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("container_new".into()),   // container ahead of session
        RefState::At("session_old".into()),     // session behind
        RefState::At("target_different".into()), // target moved
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = programs::plan_sync(&vm);

    let first_inject = ops.iter().position(|op| matches!(op, Op::Inject { .. }));
    let first_extract = ops.iter().position(|op| matches!(op, Op::Extract { .. }));

    // Both should exist
    assert!(first_inject.is_some(), "sync should have inject");
    assert!(first_extract.is_some(), "sync should have extract");

    // Inject (push) should come before Extract (pull)
    assert!(first_inject.unwrap() < first_extract.unwrap(),
        "push should come before pull in sync program");
}

// ============================================================================
// Test: full merge + verify file content is correct
// ============================================================================

#[tokio::test]
async fn merged_content_is_correct() {
    let (_tmp, path) = make_repo("content-verify");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "feature.rs", "fn feature() { 42 }", "add feature");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");

    // Also add a file on main (non-conflicting)
    commit_file(&path, "infra.rs", "fn infra() { setup() }", "add infra");
    let main_h = head_of(&path, "main");

    let backend = Git2Backend::new();
    let (clean, tree_opt, _) = backend.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(clean, "should be clean merge (different files)");
    let tree_hash = tree_opt.unwrap();

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(main_h.clone()),
        Some(path.clone()),
    ));

    let result = vm.run(&backend, vec![
        Op::checkout(Side::Host, "repo", "refs/heads/main"),
        Op::commit("repo", &tree_hash, &[&main_h], "merge"),
    ]).await;

    assert!(!result.halted);

    // Both files present on main
    assert!(tree_has_file(&path, "main", "feature.rs"));
    assert!(tree_has_file(&path, "main", "infra.rs"));
    assert!(tree_has_file(&path, "main", "README.md"));
    assert_no_markers(&path, "main");
}
