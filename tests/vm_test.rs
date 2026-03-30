//! VM tests — pure state transition tests against MockBackend.
//!
//! No git, no Docker. Construct VM state, execute ops, assert state changes.

mod common;
use common::*;
use git_sandbox::vm::*;
use std::path::PathBuf;

// ============================================================================
// Helpers
// ============================================================================

fn test_vm() -> SyncVM {
    SyncVM::new("test-session", "main")
}

fn repo_with_refs(container: &str, session: &str, target: &str) -> RepoVM {
    RepoVM::from_refs(
        RefState::At(container.into()),
        RefState::At(session.into()),
        RefState::At(target.into()),
        Some(PathBuf::from("/tmp/test-repo")),
    )
}

fn repo_absent() -> RepoVM {
    RepoVM::empty(Some(PathBuf::from("/tmp/test-repo")))
}

// ============================================================================
// Precondition tests
// ============================================================================

#[tokio::test]
async fn precondition_ref_read_requires_repo_in_vm() {
    let vm = test_vm();
    let op = Op::ref_read(Side::Host, "missing-repo", "refs/heads/main");
    assert!(op.check_preconditions(&vm).is_err());
}

#[tokio::test]
async fn precondition_ref_read_passes_when_repo_exists() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    let op = Op::ref_read(Side::Host, "alpha", "refs/heads/main");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[tokio::test]
async fn precondition_ref_write_container_rejects_dirty() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.container_clean = false;
    vm.set_repo("alpha", repo);

    let op = Op::ref_write(Side::Container, "alpha", "HEAD", "ddd");
    assert!(op.check_preconditions(&vm).is_err());
}

#[tokio::test]
async fn precondition_ref_write_host_passes_when_clean() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    let op = Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[tokio::test]
async fn precondition_checkout_rejects_host_mid_merge() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Merging;
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Host, "alpha", "refs/heads/main");
    assert!(op.check_preconditions(&vm).is_err());
}

#[tokio::test]
async fn precondition_checkout_container_passes_during_host_merge() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Merging;
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Container, "alpha", "HEAD");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[tokio::test]
async fn precondition_commit_rejects_unresolved_conflicts() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Conflicted;
    vm.set_repo("alpha", repo);

    let op = Op::commit("alpha", "tree123", &["ccc"], "msg");
    assert!(op.check_preconditions(&vm).is_err());
}

#[tokio::test]
async fn precondition_bundle_create_requires_container_present() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_absent());

    let op = Op::bundle_create("alpha");
    assert!(op.check_preconditions(&vm).is_err());
}

#[tokio::test]
async fn precondition_bundle_create_passes_with_container() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    let op = Op::bundle_create("alpha");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[tokio::test]
async fn precondition_try_merge_requires_repo() {
    let vm = test_vm();
    let op = Op::TryMerge {
        repo: "missing".into(),
        ours: "aaa".into(),
        theirs: "bbb".into(),
        on_clean: vec![],
        on_conflict: vec![],
        on_error: vec![],
    };
    assert!(op.check_preconditions(&vm).is_err());
}

#[tokio::test]
async fn precondition_confirm_always_passes() {
    let vm = test_vm();
    let op = Op::confirm("proceed?");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[tokio::test]
async fn precondition_interactive_session_always_passes() {
    let vm = test_vm();
    let op = Op::InteractiveSession {
        prompt: Some("hello".into()),
        on_exit: vec![],
    };
    assert!(op.check_preconditions(&vm).is_ok());
}

// ============================================================================
// Postcondition tests
// ============================================================================

#[tokio::test]
async fn postcondition_ref_write_updates_container() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::ref_write(Side::Container, "alpha", "HEAD", "ddd");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().container, RefState::At("ddd".into()));
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}

#[tokio::test]
async fn postcondition_ref_write_host_session_branch() {
    let mut vm = SyncVM::new("mysession", "main");
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::ref_write(Side::Host, "alpha", "refs/heads/mysession", "eee");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("eee".into()));
    assert_eq!(vm.repo("alpha").unwrap().container, RefState::At("aaa".into()));
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}

#[tokio::test]
async fn postcondition_ref_write_host_target_branch() {
    let mut vm = SyncVM::new("mysession", "main");
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::ref_write(Side::Host, "alpha", "refs/heads/main", "fff");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("fff".into()));
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
}

#[tokio::test]
async fn postcondition_bundle_fetch_updates_session() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::bundle_fetch("alpha", "/tmp/bundle");
    op.apply_postconditions(&mut vm, &OpResult::Hash("fetched123".into()));

    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("fetched123".into()));
}

#[tokio::test]
async fn postcondition_checkout_clears_host_merge_state() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Merging;
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Host, "alpha", "refs/heads/main");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().host_merge_state, HostMergeState::Clean);
}

#[tokio::test]
async fn postcondition_checkout_container_clears_conflict() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.conflict = ConflictState::Markers(vec!["file.rs".into()]);
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Container, "alpha", "HEAD");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().conflict, ConflictState::Clean);
}

#[tokio::test]
async fn postcondition_commit_updates_target() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::commit("alpha", "tree123", &["ccc"], "squash merge");
    op.apply_postconditions(&mut vm, &OpResult::Hash("new_commit".into()));

    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("new_commit".into()));
    assert_eq!(vm.repo("alpha").unwrap().host_merge_state, HostMergeState::Clean);
}

#[tokio::test]
async fn postcondition_agent_run_success_updates_container() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.conflict = ConflictState::Markers(vec!["conflict.rs".into()]);
    vm.set_repo("alpha", repo);

    let op = Op::AgentRun {
        repo: "alpha".into(),
        task: AgentTask::ResolveConflicts { files: vec!["conflict.rs".into()] },
        context: String::new(),
        on_success: vec![],
        on_failure: vec![],
    };
    op.apply_postconditions(&mut vm, &OpResult::AgentCompleted {
        resolved: true,
        description: Some("fixed".into()),
        new_head: Some("resolved_head".into()),
    });

    assert_eq!(vm.repo("alpha").unwrap().conflict, ConflictState::Resolved);
    assert_eq!(vm.repo("alpha").unwrap().container, RefState::At("resolved_head".into()));
}

#[tokio::test]
async fn postcondition_agent_run_failure_keeps_conflict() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.conflict = ConflictState::Markers(vec!["conflict.rs".into()]);
    vm.set_repo("alpha", repo);

    let op = Op::AgentRun {
        repo: "alpha".into(),
        task: AgentTask::ResolveConflicts { files: vec![] },
        context: String::new(),
        on_success: vec![],
        on_failure: vec![],
    };
    op.apply_postconditions(&mut vm, &OpResult::AgentCompleted {
        resolved: false,
        description: None,
        new_head: None,
    });

    assert!(matches!(vm.repo("alpha").unwrap().conflict, ConflictState::Markers(_)));
    assert_eq!(vm.repo("alpha").unwrap().container, RefState::At("aaa".into()));
}

#[tokio::test]
async fn postcondition_interactive_session_invalidates_all_state() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    vm.set_repo("beta", repo_with_refs("ddd", "eee", "fff"));

    let op = Op::InteractiveSession { prompt: None, on_exit: vec![] };
    op.apply_postconditions(&mut vm, &OpResult::SessionExited { exit_code: 0 });

    assert_eq!(vm.repo("alpha").unwrap().container, RefState::Absent);
    assert_eq!(vm.repo("beta").unwrap().container, RefState::Absent);
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
    assert_eq!(vm.repo("beta").unwrap().target, RefState::At("fff".into()));
}

#[tokio::test]
async fn postcondition_read_ops_dont_change_state() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let before = vm.repo("alpha").unwrap().clone();

    Op::ref_read(Side::Host, "alpha", "refs/heads/main")
        .apply_postconditions(&mut vm, &OpResult::Hash("aaa".into()));
    Op::TreeCompare { repo: "alpha".into(), a: "aaa".into(), b: "bbb".into() }
        .apply_postconditions(&mut vm, &OpResult::Comparison { identical: false, files_changed: 5 });
    Op::AncestryCheck { repo: "alpha".into(), a: "aaa".into(), b: "bbb".into() }
        .apply_postconditions(&mut vm, &OpResult::Ancestry(AncestryResult::Same));
    Op::MergeTrees { repo: "alpha".into(), ours: "aaa".into(), theirs: "bbb".into() }
        .apply_postconditions(&mut vm, &OpResult::MergeResult { clean: true, tree: Some("t".into()), conflicts: vec![] });
    Op::confirm("test")
        .apply_postconditions(&mut vm, &OpResult::UserDecision(true));

    let after = vm.repo("alpha").unwrap();
    assert_eq!(before.container, after.container);
    assert_eq!(before.session, after.session);
    assert_eq!(before.target, after.target);
}

// ============================================================================
// Trace + state construction tests
// ============================================================================

#[tokio::test]
async fn trace_records_operations() {
    let mut vm = test_vm();
    vm.record(Op::ref_read(Side::Host, "alpha", "refs/heads/main"), OpOutcome::Ok);
    vm.record(Op::confirm("proceed?"), OpOutcome::OkWithValue("yes".into()));
    assert_eq!(vm.trace.len(), 2);
}

#[tokio::test]
async fn repo_vm_empty_is_all_absent() {
    let repo = RepoVM::empty(None);
    assert_eq!(repo.container, RefState::Absent);
    assert_eq!(repo.session, RefState::Absent);
    assert_eq!(repo.target, RefState::Absent);
}

#[tokio::test]
async fn repo_vm_from_refs_sets_heads() {
    let repo = repo_with_refs("aaa", "bbb", "ccc");
    assert_eq!(repo.container, RefState::At("aaa".into()));
    assert_eq!(repo.session, RefState::At("bbb".into()));
    assert_eq!(repo.target, RefState::At("ccc".into()));
}

#[tokio::test]
async fn ref_state_hash_returns_value() {
    assert_eq!(RefState::At("abc".into()).hash(), Some("abc"));
    assert_eq!(RefState::Absent.hash(), None);
}

#[tokio::test]
async fn sync_vm_manages_repos() {
    let mut vm = test_vm();
    assert!(vm.repo("alpha").is_none());
    vm.set_repo("alpha", repo_with_refs("a", "b", "c"));
    assert!(vm.repo("alpha").is_some());
}

// ============================================================================
// Interpreter tests — run ops through the VM with MockBackend
// ============================================================================

#[tokio::test]
async fn interpreter_runs_primitive_sequence() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::BundleCreate, MockResult::Hash("/tmp/alpha.bundle".into()));
    mock.on(CallMatcher::BundleFetch, MockResult::Hash("fetched_abc".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "old_session", "ccc"));

    let result = vm.run(&mock, vec![
        Op::bundle_create("alpha"),
        Op::bundle_fetch("alpha", "/tmp/alpha.bundle"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/test-session", "fetched_abc"),
    ]).await;

    assert_eq!(result.succeeded(), 3);
    assert!(!result.halted);
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("fetched_abc".into()));

    let calls = mock.calls();
    assert_eq!(calls.len(), 3);
    assert!(matches!(calls[0], RecordedCall::BundleCreate { .. }));
    assert!(matches!(calls[1], RecordedCall::BundleFetch { .. }));
    assert!(matches!(calls[2], RecordedCall::RefWrite { .. }));
}

#[tokio::test]
async fn interpreter_halts_on_precondition_failure() {
    let mock = MockBackend::new();
    let mut vm = test_vm();

    let result = vm.run(&mock, vec![
        Op::bundle_create("alpha"),
    ]).await;

    assert!(result.halted);
    assert_eq!(result.succeeded(), 0);
    assert!(mock.calls().is_empty());
}

#[tokio::test]
async fn interpreter_halts_on_backend_error() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::BundleCreate, MockResult::Error("disk full".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::bundle_create("alpha"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd"),
    ]).await;

    assert!(result.halted);
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
}

#[tokio::test]
async fn interpreter_confirm_decline_halts() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::PromptUser, MockResult::Bool(false));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::confirm("proceed?"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd"),
    ]).await;

    assert!(result.halted);
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}

#[tokio::test]
async fn interpreter_confirm_accept_continues() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::PromptUser, MockResult::Bool(true));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::confirm("proceed?"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd"),
    ]).await;

    assert!(!result.halted);
    assert_eq!(result.succeeded(), 2);
}

#[tokio::test]
async fn interpreter_try_merge_follows_clean_path() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::MergeTrees, MockResult::MergeClean("merged_tree".into()));
    mock.on(CallMatcher::Commit, MockResult::Hash("new_commit".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::TryMerge {
            repo: "alpha".into(),
            ours: "ccc".into(),
            theirs: "aaa".into(),
            on_clean: vec![
                Op::commit("alpha", "merged_tree", &["ccc"], "squash merge"),
            ],
            on_conflict: vec![
                Op::confirm("should NOT run"),
            ],
            on_error: vec![],
        },
    ]).await;

    assert!(!result.halted);
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("new_commit".into()));
    assert!(!mock.was_called(&CallMatcher::PromptUser));
    assert!(mock.was_called(&CallMatcher::Commit));
}

#[tokio::test]
async fn interpreter_try_merge_follows_conflict_path() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::MergeTrees, MockResult::MergeConflict(vec!["shared.txt".into()]));
    mock.on(CallMatcher::AgentRun, MockResult::Hash("resolved_head".into()));
    mock.on(CallMatcher::BundleCreate, MockResult::Hash("/tmp/bundle".into()));

    let mut vm = test_vm();
    vm.set_repo("gamma", repo_with_refs("container_h", "session_h", "target_h"));

    let result = vm.run(&mock, vec![
        Op::TryMerge {
            repo: "gamma".into(),
            ours: "target_h".into(),
            theirs: "container_h".into(),
            on_clean: vec![
                Op::commit("gamma", "TREE", &["target_h"], "should NOT run"),
            ],
            on_conflict: vec![
                Op::AgentRun {
                    repo: "gamma".into(),
                    task: AgentTask::ResolveConflicts { files: vec!["shared.txt".into()] },
                    context: String::new(),
                    on_success: vec![
                        Op::bundle_create("gamma"),
                    ],
                    on_failure: vec![],
                },
            ],
            on_error: vec![],
        },
    ]).await;

    assert!(!result.halted);
    assert!(mock.was_called(&CallMatcher::AgentRun));
    assert!(mock.was_called(&CallMatcher::BundleCreate));
    assert!(!mock.was_called(&CallMatcher::Commit));
    assert_eq!(vm.repo("gamma").unwrap().conflict, ConflictState::Resolved);
}

#[tokio::test]
async fn interpreter_agent_failure_follows_failure_path() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::AgentRun, MockResult::Bool(false));

    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.conflict = ConflictState::Markers(vec!["file.rs".into()]);
    vm.set_repo("alpha", repo);

    let result = vm.run(&mock, vec![
        Op::AgentRun {
            repo: "alpha".into(),
            task: AgentTask::ResolveConflicts { files: vec!["file.rs".into()] },
            context: String::new(),
            on_success: vec![
                Op::bundle_create("alpha"),
            ],
            on_failure: vec![
                Op::checkout(Side::Container, "alpha", "HEAD"),
            ],
        },
    ]).await;

    assert!(!result.halted);
    assert!(!mock.was_called(&CallMatcher::BundleCreate));
    assert!(mock.was_called(&CallMatcher::Checkout));
    assert_eq!(vm.repo("alpha").unwrap().conflict, ConflictState::Clean);
}

#[tokio::test]
async fn interpreter_interactive_session_invalidates_state() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::InteractiveSession, MockResult::ContainerExited(0));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    vm.set_repo("beta", repo_with_refs("ddd", "eee", "fff"));

    let result = vm.run(&mock, vec![
        Op::InteractiveSession { prompt: Some("hello".into()), on_exit: vec![] },
    ]).await;

    assert!(!result.halted);
    assert_eq!(vm.repo("alpha").unwrap().container, RefState::Absent);
    assert_eq!(vm.repo("beta").unwrap().container, RefState::Absent);
}

#[tokio::test]
async fn interpreter_state_unchanged_on_backend_error() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::RefWrite, MockResult::Error("permission denied".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "new_hash"),
    ]).await;

    assert!(result.halted);
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}

// ============================================================================
// Error path tests — what happens when ops fail
// ============================================================================

#[tokio::test]
async fn inject_error_halts_program_and_preserves_state() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::Inject, MockResult::Error("Docker container wait error".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "aaa", "bbb"));

    let result = vm.run(&mock, vec![
        Op::Inject { repo: "alpha".into(), branch: "main".into() },
        Op::Extract { repo: "alpha".into(), session_branch: "test-session".into() },
    ]).await;

    // Program halted on inject error
    assert!(result.halted);
    assert_eq!(result.succeeded(), 0);
    assert_eq!(result.failed(), 1);

    // Extract never ran
    assert!(!mock.was_called(&CallMatcher::Extract));

    // VM state unchanged — inject failed, no postconditions applied
    assert_eq!(vm.repo("alpha").unwrap().container, RefState::At("aaa".into()));
    // Container should NOT be Stale (inject didn't succeed)
    assert_ne!(vm.repo("alpha").unwrap().container, RefState::Stale);

    // Error message should be visible
    let error_msg = result.outcomes.first()
        .and_then(|o| match &o.result {
            StepResult::BackendError(e) => Some(e.as_str()),
            _ => None,
        });
    assert!(error_msg.is_some(), "error message should be present");
    assert!(error_msg.unwrap().contains("Docker container wait error"));
}

#[tokio::test]
async fn extract_error_halts_program() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::Extract, MockResult::Error("bundle creation failed: disk full".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::Extract { repo: "alpha".into(), session_branch: "test-session".into() },
        Op::commit("alpha", "tree", &["ccc"], "should not run"),
    ]).await;

    assert!(result.halted);
    assert_eq!(result.succeeded(), 0);
    // Session unchanged
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
}

#[tokio::test]
async fn force_inject_error_halts_program() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::ForceInject, MockResult::Error("container not found".into()));

    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "aaa", "bbb");
    repo.container_clean = false; // dirty — force inject needed
    vm.set_repo("alpha", repo);

    let result = vm.run(&mock, vec![
        Op::ForceInject { repo: "alpha".into(), branch: "main".into() },
    ]).await;

    assert!(result.halted);
    assert_eq!(result.failed(), 1);
}

#[tokio::test]
async fn push_program_with_inject_error_reports_clearly() {
    // Simulates the real scenario: push generates inject + extract,
    // inject fails with Docker error, extract never runs
    let mock = MockBackend::new();
    mock.on(CallMatcher::Inject, MockResult::Error("Docker API error: Docker container wait error".into()));

    let mut vm = test_vm();
    vm.set_repo("repo", repo_with_refs("same", "same", "ahead"));

    // This is what plan_push generates for target-ahead
    let ops = vec![
        Op::Inject { repo: "repo".into(), branch: "main".into() },
        Op::Extract { repo: "repo".into(), session_branch: "test-session".into() },
    ];

    let result = vm.run(&mock, ops).await;

    assert!(result.halted);
    assert_eq!(result.succeeded(), 0);
    assert_eq!(result.failed(), 1);
    assert!(!mock.was_called(&CallMatcher::Extract), "extract should not run after inject failure");

    // The halt reason should tell the user what happened
    assert!(result.halt_reason.as_ref().unwrap().contains("Docker"));
}

#[tokio::test]
async fn try_merge_error_follows_error_path() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::MergeTrees, MockResult::Error("git2 error: corrupt object".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::TryMerge {
            repo: "alpha".into(),
            ours: "ccc".into(),
            theirs: "aaa".into(),
            on_clean: vec![
                Op::commit("alpha", "tree", &["ccc"], "should NOT run"),
            ],
            on_conflict: vec![],
            on_error: vec![
                Op::checkout(Side::Host, "alpha", "refs/heads/main"),
            ],
        },
    ]).await;

    // Error path was taken
    assert!(mock.was_called(&CallMatcher::Checkout), "error path should run checkout");
    assert!(!mock.was_called(&CallMatcher::Commit), "clean path should NOT run");
}

#[tokio::test]
async fn multiple_repos_first_failure_doesnt_block_independent_ops() {
    // In a real sync, repos are independent — failure in one shouldn't
    // block the others. But the current interpreter halts on first error.
    // This test documents that behavior.
    let mock = MockBackend::new();
    mock.on(CallMatcher::Inject, MockResult::Error("repo-a failed".into()));

    let mut vm = test_vm();
    vm.set_repo("repo-a", repo_with_refs("a1", "a1", "a2"));
    vm.set_repo("repo-b", repo_with_refs("b1", "b1", "b2"));

    let ops = vec![
        Op::Inject { repo: "repo-a".into(), branch: "main".into() },
        Op::Inject { repo: "repo-b".into(), branch: "main".into() },
    ];

    let result = vm.run(&mock, ops).await;

    // Current behavior: halts on first failure, repo-b never attempted
    assert!(result.halted);
    assert_eq!(result.outcomes.len(), 1, "only first op attempted");
    // This is a known limitation — independent repos should be able
    // to continue past failures. Tracked for future improvement.
}

// ============================================================================
// Git2Backend tests — VM ops against real temp repos (no Docker)
// Repos created via tempfile::TempDir, cleaned up on drop.
// ============================================================================

use git2::Repository;
use std::path::Path;

#[tokio::test]
async fn git2_ref_read_and_write() {
    let (_tmp, path) = make_repo("ref-rw");
    let backend = Git2Backend::new();

    let head = backend.ref_read(&path, "refs/heads/main").await.unwrap();
    assert!(head.is_some());

    let missing = backend.ref_read(&path, "refs/heads/nonexistent").await.unwrap();
    assert!(missing.is_none());
}

#[tokio::test]
async fn git2_ancestry_same() {
    let (_tmp, path) = make_repo("anc-same");
    let backend = Git2Backend::new();
    let h = head_of(&path, "main");
    assert_eq!(backend.ancestry_check(&path, &h, &h).await.unwrap(), AncestryResult::Same);
}

#[tokio::test]
async fn git2_ancestry_ahead() {
    let (_tmp, path) = make_repo("anc-ahead");
    let before = head_of(&path, "main");
    commit_file(&path, "a.txt", "a", "c1");
    commit_file(&path, "b.txt", "b", "c2");
    let after = head_of(&path, "main");

    let backend = Git2Backend::new();
    let result = backend.ancestry_check(&path, &before, &after).await.unwrap();
    assert!(matches!(result, AncestryResult::AIsAncestorOfB { distance: 2 }));
}

#[tokio::test]
async fn git2_ancestry_diverged() {
    let (_tmp, path) = make_repo("anc-div");
    git_branch(&path, "session");
    commit_file(&path, "main.txt", "m", "main");
    let main_h = head_of(&path, "main");

    git_switch(&path, "session");
    commit_file(&path, "session.txt", "s", "session");
    let session_h = head_of(&path, "session");

    let backend = Git2Backend::new();
    let result = backend.ancestry_check(&path, &main_h, &session_h).await.unwrap();
    assert!(matches!(result, AncestryResult::Diverged { a_ahead: 1, b_ahead: 1, .. }));
}

#[tokio::test]
async fn git2_merge_trees_clean() {
    let (_tmp, path) = make_repo("merge-clean");
    git_branch(&path, "session");
    commit_file(&path, "main.txt", "m", "main");
    let main_h = head_of(&path, "main");

    git_switch(&path, "session");
    commit_file(&path, "session.txt", "s", "session");
    let session_h = head_of(&path, "session");

    let backend = Git2Backend::new();
    let (clean, tree, conflicts) = backend.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(clean);
    assert!(tree.is_some());
    assert!(conflicts.is_empty());
}

#[tokio::test]
async fn git2_merge_trees_conflict() {
    let (_tmp, path) = make_repo("merge-conflict");
    commit_file(&path, "shared.txt", "original", "base");
    git_branch(&path, "session");

    commit_file(&path, "shared.txt", "main version", "main");
    let main_h = head_of(&path, "main");

    git_switch(&path, "session");
    commit_file(&path, "shared.txt", "session version", "session");
    let session_h = head_of(&path, "session");

    let backend = Git2Backend::new();
    let (clean, _, conflicts) = backend.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(!clean);
    assert!(conflicts.contains(&"shared.txt".to_string()));
}

#[tokio::test]
async fn git2_vm_merge_advances_target() {
    let (_tmp, path) = make_repo("vm-merge");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "a.txt", "aaa", "c1");
    commit_file(&path, "b.txt", "bbb", "c2");
    git_switch(&path, "main");

    let main_before = head_of(&path, "main");
    let session_h = head_of(&path, "session");

    let backend = Git2Backend::new();

    // Merge trees to get the merged tree hash
    let (clean, tree, _) = backend.merge_trees(&path, &main_before, &session_h).await.unwrap();
    assert!(clean);
    let tree_hash = tree.unwrap();

    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        RefState::At(session_h.clone()),
        RefState::At(session_h.clone()),
        RefState::At(main_before.clone()),
        Some(path.clone()),
    ));

    let result = vm.run(&backend, vec![
        Op::checkout(Side::Host, "repo", "refs/heads/main"),
        Op::commit("repo", &tree_hash, &[&main_before], "squash merge"),
    ]).await;

    assert!(!result.halted, "halted: {:?}", result.halt_reason);
    let main_after = head_of(&path, "main");
    assert_ne!(main_before, main_after, "target should advance");
    assert_no_markers(&path, "main");
}

#[tokio::test]
async fn git2_vm_conflict_target_unchanged() {
    let (_tmp, path) = make_repo("vm-conflict");
    commit_file(&path, "shared.txt", "original", "base");
    git_branch(&path, "session");

    commit_file(&path, "shared.txt", "main version", "main edit");
    let main_h = head_of(&path, "main");

    git_switch(&path, "session");
    commit_file(&path, "shared.txt", "session version", "session edit");
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
            on_conflict: vec![],
            on_error: vec![],
        },
    ]).await;

    assert!(!result.halted);
    assert_eq!(head_of(&path, "main"), main_h, "target unchanged after conflict");
    assert_no_markers(&path, "main");
}

#[tokio::test]
async fn git2_temp_repo_cleaned_up_on_drop() {
    let path = {
        let (_tmp, path) = make_repo("cleanup-test");
        assert!(path.join(".git").exists());
        path
    };
    assert!(!path.exists(), "temp repo should be gone after drop");
}

// ── ref_write: write a ref and read it back ──

#[tokio::test]
async fn git2_ref_write_creates_branch() {
    let (_tmp, path) = make_repo("ref-write");
    let backend = Git2Backend::new();

    let main_h = head_of(&path, "main");
    // Write a new branch ref pointing at main's HEAD
    backend.ref_write(&path, "refs/heads/new-branch", &main_h).await.unwrap();

    // Read it back
    let read_back = backend.ref_read(&path, "refs/heads/new-branch").await.unwrap();
    assert_eq!(read_back, Some(main_h));
}

#[tokio::test]
async fn git2_ref_write_updates_existing() {
    let (_tmp, path) = make_repo("ref-update");
    let backend = Git2Backend::new();

    let old_head = head_of(&path, "main");
    commit_file(&path, "new.txt", "content", "advance");
    let new_head = head_of(&path, "main");
    assert_ne!(old_head, new_head);

    // Create a branch at old HEAD
    backend.ref_write(&path, "refs/heads/marker", &old_head).await.unwrap();
    assert_eq!(backend.ref_read(&path, "refs/heads/marker").await.unwrap(), Some(old_head.clone()));

    // Update it to new HEAD
    backend.ref_write(&path, "refs/heads/marker", &new_head).await.unwrap();
    assert_eq!(backend.ref_read(&path, "refs/heads/marker").await.unwrap(), Some(new_head));
}

// ── tree_compare ──

#[tokio::test]
async fn git2_tree_compare_identical() {
    let (_tmp, path) = make_repo("tree-same");
    let backend = Git2Backend::new();
    let h = head_of(&path, "main");

    let (identical, files) = backend.tree_compare(&path, &h, &h).await.unwrap();
    assert!(identical);
    assert_eq!(files, 0);
}

#[tokio::test]
async fn git2_tree_compare_different() {
    let (_tmp, path) = make_repo("tree-diff");
    let before = head_of(&path, "main");
    commit_file(&path, "new.txt", "content", "add file");
    let after = head_of(&path, "main");

    let backend = Git2Backend::new();
    let (identical, files) = backend.tree_compare(&path, &before, &after).await.unwrap();
    assert!(!identical);
    assert_eq!(files, 1);
}

// ── checkout ──

#[tokio::test]
async fn git2_checkout_switches_worktree() {
    let (_tmp, path) = make_repo("checkout");
    commit_file(&path, "main-only.txt", "main", "main file");
    git_branch(&path, "other");
    git_switch(&path, "other");
    commit_file(&path, "other-only.txt", "other", "other file");

    // Worktree has other-only.txt
    assert!(path.join("other-only.txt").exists());

    let backend = Git2Backend::new();
    backend.checkout(&path, "refs/heads/main").await.unwrap();

    // Worktree now has main-only.txt, not other-only.txt
    assert!(path.join("main-only.txt").exists());
    assert!(!path.join("other-only.txt").exists());
}

// ── commit ──

#[tokio::test]
async fn git2_commit_creates_new_commit() {
    let (_tmp, path) = make_repo("commit");
    let backend = Git2Backend::new();

    // Add a file to the index and write a tree
    std::fs::write(path.join("new.txt"), "hello").unwrap();
    let repo = Repository::open(&path).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("new.txt")).unwrap();
    index.write().unwrap();
    let tree_oid = index.write_tree().unwrap();
    let tree_hash = tree_oid.to_string();
    let parent = head_of(&path, "main");

    let new_hash = backend.commit(&path, &tree_hash, &[parent.clone()], "test commit").await.unwrap();
    assert_ne!(new_hash, parent);

    // New commit is HEAD
    let current_head = head_of(&path, "main");
    assert_eq!(current_head, new_hash);
}

// ── error cases ──

#[tokio::test]
async fn git2_ref_read_bad_path_returns_error() {
    let backend = Git2Backend::new();
    let result = backend.ref_read(Path::new("/nonexistent/repo"), "refs/heads/main").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn git2_ref_write_bad_hash_returns_error() {
    let (_tmp, path) = make_repo("bad-hash");
    let backend = Git2Backend::new();
    let result = backend.ref_write(&path, "refs/heads/test", "not-a-valid-hash").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn git2_merge_trees_bad_hash_returns_error() {
    let (_tmp, path) = make_repo("bad-merge");
    let backend = Git2Backend::new();
    let result = backend.merge_trees(&path, "0000000000000000000000000000000000000000", "0000000000000000000000000000000000000000").await;
    assert!(result.is_err());
}

// ── Docker ops return errors ──

#[tokio::test]
async fn git2_docker_ops_return_errors() {
    let backend = Git2Backend::new();
    assert!(backend.bundle_create("session", "repo").await.is_err());
    assert!(backend.bundle_fetch(Path::new("/tmp"), "/tmp/b").await.is_err());
    assert!(backend.run_container("img", "script", &[]).await.is_err());
    assert!(backend.extract("session", "repo", Path::new("/tmp"), "session-branch").await.is_err());
    assert!(backend.inject("session", "repo", Path::new("/tmp"), "main").await.is_err());
    assert!(backend.force_inject("session", "repo", Path::new("/tmp"), "main").await.is_err());
    assert!(backend.agent_run(&AgentTask::Work, "", &[]).await.is_err());
    assert!(backend.interactive_session(None, &[]).await.is_err());
    // prompt_user auto-confirms in Git2Backend
    assert_eq!(backend.prompt_user("test").await.unwrap(), true);
}

// ============================================================================
// Program generator tests — plan_push, plan_pull, plan_sync
// ============================================================================

#[tokio::test]
async fn plan_push_injects_when_target_differs() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("aaa".into()),  // container
        RefState::At("aaa".into()),  // session (same as container)
        RefState::At("bbb".into()),  // target ahead
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = plan_push(&vm);
    assert!(!ops.is_empty(), "should have inject ops");
    // Should contain an Inject and an Extract (re-extract after inject)
    assert!(ops.iter().any(|op| matches!(op, Op::Inject { .. })));
    assert!(ops.iter().any(|op| matches!(op, Op::Extract { .. })));
}

#[tokio::test]
async fn plan_push_skips_when_in_sync() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),  // all same
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = plan_push(&vm);
    assert!(ops.is_empty(), "nothing to push when in sync");
}

#[tokio::test]
async fn plan_push_skips_host_dirty() {
    let mut vm = SyncVM::new("session", "main");
    let mut repo = RepoVM::from_refs(
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),
        RefState::At("bbb".into()),
        Some(PathBuf::from("/tmp/alpha")),
    );
    repo.host_clean = false;  // host dirty doesn't block push
    vm.set_repo("alpha", repo);

    let ops = plan_push(&vm);
    assert!(!ops.is_empty(), "host dirty should not block push");
    assert!(ops.iter().any(|op| matches!(op, Op::Inject { .. })));
}

#[tokio::test]
async fn plan_pull_extracts_when_container_ahead() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("bbb".into()),  // container ahead
        RefState::At("aaa".into()),  // session behind
        RefState::At("aaa".into()),  // target = session
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = plan_pull(&vm);
    assert!(ops.iter().any(|op| matches!(op, Op::Extract { .. })), "should extract");
}

#[tokio::test]
async fn plan_pull_merges_when_session_ahead_of_target() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("bbb".into()),  // container
        RefState::At("bbb".into()),  // session = container
        RefState::At("aaa".into()),  // target behind
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = plan_pull(&vm);
    assert!(ops.iter().any(|op| matches!(op, Op::TryMerge { .. })), "should merge");
}

#[tokio::test]
async fn plan_pull_skips_when_in_sync() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = plan_pull(&vm);
    assert!(ops.is_empty(), "nothing to pull when in sync");
}

#[tokio::test]
async fn plan_pull_blocked_when_host_dirty() {
    let mut vm = SyncVM::new("session", "main");
    let mut repo = RepoVM::from_refs(
        RefState::At("bbb".into()),
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),
        Some(PathBuf::from("/tmp/alpha")),
    );
    repo.host_clean = false;
    vm.set_repo("alpha", repo);

    let ops = plan_pull(&vm);
    assert!(ops.is_empty(), "host dirty should block pull");
}

#[tokio::test]
async fn plan_sync_push_before_pull() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("bbb".into()),  // container ahead (pull needed)
        RefState::At("aaa".into()),  // session
        RefState::At("ccc".into()),  // target different (push needed)
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = plan_sync(&vm);
    // Find first Inject (push) and first Extract (pull)
    let first_inject = ops.iter().position(|op| matches!(op, Op::Inject { .. }));
    let first_extract = ops.iter().position(|op| matches!(op, Op::Extract { .. }));

    // If both exist, push (Inject) should come before pull (Extract)
    if let (Some(push_idx), Some(pull_idx)) = (first_inject, first_extract) {
        assert!(push_idx < pull_idx, "push ops should come before pull ops in sync");
    }
}

#[tokio::test]
async fn plan_sync_empty_when_in_sync() {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),
        RefState::At("aaa".into()),
        Some(PathBuf::from("/tmp/alpha")),
    ));

    let ops = plan_sync(&vm);
    assert!(ops.is_empty());
}

// ============================================================================
// Compound builder tests
// ============================================================================

#[tokio::test]
async fn ops_extract_produces_bundle_sequence() {
    let ops = ops_extract("alpha", "session");
    assert_eq!(ops.len(), 3);
    assert!(matches!(ops[0], Op::BundleCreate { .. }));
    assert!(matches!(ops[1], Op::BundleFetch { .. }));
    assert!(matches!(ops[2], Op::RefWrite { .. }));
}

#[tokio::test]
async fn ops_inject_produces_run_container() {
    let ops = ops_inject("alpha", "main");
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], Op::RunContainer { .. }));
}

#[tokio::test]
async fn ops_merge_produces_try_merge() {
    let op = ops_merge("alpha", "session_h", "target_h", "main", true);
    assert!(matches!(op, Op::TryMerge { .. }));
    if let Op::TryMerge { on_clean, on_error, .. } = &op {
        assert!(!on_clean.is_empty(), "clean path should have ops");
        assert!(!on_error.is_empty(), "error path should have cleanup");
    }
}

#[tokio::test]
async fn ops_clone_produces_run_container() {
    let ops = ops_clone("alpha");
    assert_eq!(ops.len(), 1);
    assert!(matches!(ops[0], Op::RunContainer { .. }));
}

#[tokio::test]
async fn ops_reconcile_with_agent_has_agent_run() {
    let ops = ops_reconcile_with_agent("alpha", "session", "target_h", "main", vec!["file.rs".into()]);
    assert!(ops.iter().any(|op| matches!(op, Op::AgentRun { .. })), "should have agent run");
}

// ============================================================================
// Display tests
// ============================================================================

#[tokio::test]
async fn display_primitive_ops() {
    let ops = vec![
        Op::ref_read(Side::Host, "alpha", "refs/heads/main"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "abc1234def"),
        Op::bundle_create("alpha"),
        Op::checkout(Side::Container, "alpha", "HEAD"),
        Op::confirm("proceed?"),
    ];

    for op in &ops {
        let s = format!("{}", op);
        assert!(!s.is_empty(), "display should produce output for {:?}", op);
    }
}

#[tokio::test]
async fn display_compound_ops() {
    let op = ops_merge("alpha", "session_h", "target_h", "main", true);
    let s = format!("{}", op);
    assert!(s.contains("try-merge"), "display: {}", s);
}

#[tokio::test]
async fn render_program_shows_numbered_steps() {
    let ops = vec![
        Op::bundle_create("alpha"),
        Op::bundle_fetch("alpha", "/tmp/bundle"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/session", "abc"),
    ];
    let output = render_program(&ops, 0);
    assert!(output.contains("1."));
    assert!(output.contains("2."));
    assert!(output.contains("3."));
    assert!(output.contains("bundle-create"));
    assert!(output.contains("bundle-fetch"));
}

#[tokio::test]
async fn render_program_shows_compound_branches() {
    let op = ops_merge("alpha", "s", "t", "main", true);
    let output = render_program(&[op], 0);
    assert!(output.contains("try-merge"));
    assert!(output.contains("on clean:"));
    assert!(output.contains("on error:"));
}

// ============================================================================
// StagingBackend tests — transactional execution against real repos
// ============================================================================

#[tokio::test]
async fn staging_ref_write_doesnt_modify_real_branch() {
    let (_tmp, path) = make_repo("staging-rw");
    commit_file(&path, "a.txt", "a", "c1");
    let main_before = head_of(&path, "main");

    // Create a commit on a side branch (doesn't move main)
    git_branch(&path, "side");
    git_switch(&path, "side");
    let new_hash = commit_file(&path, "b.txt", "b", "c2");
    git_switch(&path, "main");
    assert_eq!(head_of(&path, "main"), main_before, "setup: main didn't move");

    let inner = Git2Backend::new();
    let staging = StagingBackend::new(inner);

    // Stage a write of the side branch hash to main
    staging.ref_write(&path, "refs/heads/main", &new_hash).await.unwrap();

    // Real main is UNCHANGED
    assert_eq!(head_of(&path, "main"), main_before, "staging should not modify real ref");

    // Staging recorded it
    let staged = staging.staged();
    assert_eq!(staged.len(), 1);
    assert_eq!(staged[0].real_ref, "refs/heads/main");
    assert_eq!(staged[0].new_hash, new_hash);

    // But ref_read through staging returns the STAGED value
    let read = staging.ref_read(&path, "refs/heads/main").await.unwrap();
    assert_eq!(read, Some(new_hash.clone()), "staging read should return staged value");
}

#[tokio::test]
async fn staging_apply_promotes_to_real_ref() {
    let (_tmp, path) = make_repo("staging-apply");
    let main_before = head_of(&path, "main");
    commit_file(&path, "a.txt", "a", "c1");
    let new_hash = head_of(&path, "main");

    // Reset main back to original
    {
        let repo = git2::Repository::open(&path).unwrap();
        let oid = git2::Oid::from_str(&main_before).unwrap();
        repo.reference("refs/heads/main", oid, true, "reset").unwrap();
    }
    assert_eq!(head_of(&path, "main"), main_before);

    let inner = Git2Backend::new();
    let staging = StagingBackend::new(inner);

    // Stage a write to main
    staging.ref_write(&path, "refs/heads/main", &new_hash).await.unwrap();
    assert_eq!(head_of(&path, "main"), main_before, "still unchanged before apply");

    // Apply
    staging.apply().unwrap();
    assert_eq!(head_of(&path, "main"), new_hash, "main should advance after apply");
}

#[tokio::test]
async fn staging_discard_cleans_up() {
    let (_tmp, path) = make_repo("staging-discard");
    let main_before = head_of(&path, "main");
    commit_file(&path, "a.txt", "a", "c1");
    let new_hash = head_of(&path, "main");

    // Reset main back
    {
        let repo = git2::Repository::open(&path).unwrap();
        let oid = git2::Oid::from_str(&main_before).unwrap();
        repo.reference("refs/heads/main", oid, true, "reset").unwrap();
    }

    let inner = Git2Backend::new();
    let staging = StagingBackend::new(inner);

    staging.ref_write(&path, "refs/heads/main", &new_hash).await.unwrap();
    assert!(!staging.staged().is_empty());

    // Discard
    staging.discard();
    assert!(staging.staged().is_empty());
    assert_eq!(head_of(&path, "main"), main_before, "main unchanged after discard");
}

#[tokio::test]
async fn staging_merge_creates_real_objects_but_doesnt_move_ref() {
    let (_tmp, path) = make_repo("staging-merge");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "feature.txt", "feature", "add feature");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");
    let main_h = head_of(&path, "main");

    let inner = Git2Backend::new();
    let staging = StagingBackend::new(inner);

    // Merge trees — creates real git objects (immutable, harmless)
    let (clean, tree_opt, _) = staging.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(clean);
    let tree_hash = tree_opt.unwrap();

    // Commit — creates real commit object
    let new_commit = staging.commit(&path, &tree_hash, &[main_h.clone()], "staged merge").await.unwrap();

    // Stage the ref write
    staging.ref_write(&path, "refs/heads/main", &new_commit).await.unwrap();

    // Main STILL hasn't moved
    assert_eq!(head_of(&path, "main"), main_h, "main unchanged before apply");

    // But the commit object exists
    let repo = git2::Repository::open(&path).unwrap();
    let oid = git2::Oid::from_str(&new_commit).unwrap();
    assert!(repo.find_commit(oid).is_ok(), "commit object should exist");

    // Apply — NOW main moves
    staging.apply().unwrap();
    assert_eq!(head_of(&path, "main"), new_commit, "main should advance after apply");
    assert_no_markers(&path, "main");
}

#[tokio::test]
async fn staging_full_pull_program_then_apply() {
    let (_tmp, path) = make_repo("staging-pull");
    git_branch(&path, "session");
    git_switch(&path, "session");
    commit_file(&path, "work.txt", "agent work", "agent commit");
    let session_h = head_of(&path, "session");
    git_switch(&path, "main");
    let main_h = head_of(&path, "main");

    let inner = Git2Backend::new();
    let staging = StagingBackend::new(inner);

    // Run a merge program through staging
    let (clean, tree, _) = staging.merge_trees(&path, &main_h, &session_h).await.unwrap();
    assert!(clean);
    let tree_hash = tree.unwrap();
    let new_commit = staging.commit(&path, &tree_hash, &[main_h.clone()], "squash merge").await.unwrap();
    staging.ref_write(&path, "refs/heads/main", &new_commit).await.unwrap();

    // Preview: main hasn't moved, but we know what it WOULD be
    let staged = staging.staged();
    assert_eq!(staged.len(), 1);
    assert_eq!(head_of(&path, "main"), main_h, "preview: main unchanged");
    eprintln!("Preview: main would move from {} to {}", &main_h[..7], &new_commit[..7]);

    // Apply
    staging.apply().unwrap();
    assert_eq!(head_of(&path, "main"), new_commit, "main advanced after apply");

    // Verify content
    assert!(tree_has_file(&path, "main", "work.txt"), "agent work should be on main");
    assert_no_markers(&path, "main");
}
