//! End-to-end VM tests — full programs against real repos.
//!
//! These verify that the VM pipeline (plan → generate → execute) produces
//! correct git state. Not mocks — real repos, real merges.

mod common;
use common::*;
use git_sandbox::vm::*;
use std::path::PathBuf;

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

// ============================================================================
// Test: Reconcile fires when BOTH container and target diverged from session
// ============================================================================

#[tokio::test]
async fn reconcile_fires_when_both_sides_changed() {
    // The scenario: an agent worked in the container (container ahead of session),
    // AND someone pushed to main (target ahead of session).
    // Both sides changed independently. This is a reconcile situation.

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("container_new_work".into()),  // agent made commits
        RefState::At("session_old".into()),         // session not yet updated
        RefState::At("target_external".into()),     // someone pushed to main
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let pull = programs::repo_pull_action(vm.repo("alpha").unwrap());
    assert!(matches!(pull, programs::PullIntent::Reconcile { .. }),
        "diverged state (container ahead + target moved) should be Reconcile, got {:?}", pull);
}

#[tokio::test]
async fn reconcile_not_fired_when_only_container_ahead() {
    // Container ahead but target hasn't moved — simple Extract, not Reconcile.
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("container_new".into()),
        RefState::At("session_old".into()),
        RefState::At("session_old".into()),  // target == session — no external work
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let pull = programs::repo_pull_action(vm.repo("alpha").unwrap());
    assert!(matches!(pull, programs::PullIntent::Extract),
        "only container ahead should be Extract, got {:?}", pull);
}

#[tokio::test]
async fn reconcile_not_fired_when_only_target_ahead() {
    // Container matches session, target moved — MergeToTarget, not Reconcile.
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("same".into()),
        RefState::At("same".into()),
        RefState::At("target_moved".into()),
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let pull = programs::repo_pull_action(vm.repo("alpha").unwrap());
    assert!(matches!(pull, programs::PullIntent::MergeToTarget),
        "only target ahead should be MergeToTarget, got {:?}", pull);
}

#[tokio::test]
async fn reconcile_generates_inject_then_extract_then_merge() {
    // When reconcile fires, plan_pull should generate: inject + extract + merge

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("container_new".into()),
        RefState::At("session_old".into()),
        RefState::At("target_external".into()),
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = programs::plan_pull(&vm);

    // Should have inject (push target into container first)
    let has_inject = ops.iter().any(|op| matches!(op, Op::Inject { .. }));
    // Should have extract (get merged result back)
    let has_extract = ops.iter().any(|op| matches!(op, Op::Extract { .. }));
    // Should have merge (merge into target)
    let has_merge = ops.iter().any(|op| matches!(op, Op::TryMerge { .. }));

    assert!(has_inject, "reconcile should inject target into container, got: {:?}", ops);
    assert!(has_extract, "reconcile should extract after inject, got: {:?}", ops);
    assert!(has_merge, "reconcile should merge into target, got: {:?}", ops);

    // Inject should come before extract
    let inject_pos = ops.iter().position(|op| matches!(op, Op::Inject { .. })).unwrap();
    let extract_pos = ops.iter().position(|op| matches!(op, Op::Extract { .. })).unwrap();
    assert!(inject_pos < extract_pos, "inject should come before extract in reconcile");
}
