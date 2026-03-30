//! VM tests — pure state transition tests against MockBackend.
//!
//! No git, no Docker. Construct VM state, execute ops, assert state changes.

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

#[test]
fn precondition_ref_read_requires_repo_in_vm() {
    let vm = test_vm();
    let op = Op::ref_read(Side::Host, "missing-repo", "refs/heads/main");
    assert!(op.check_preconditions(&vm).is_err());
}

#[test]
fn precondition_ref_read_passes_when_repo_exists() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    let op = Op::ref_read(Side::Host, "alpha", "refs/heads/main");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[test]
fn precondition_ref_write_container_rejects_dirty() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.container_clean = false;
    vm.set_repo("alpha", repo);

    let op = Op::ref_write(Side::Container, "alpha", "HEAD", "ddd");
    assert!(op.check_preconditions(&vm).is_err());
}

#[test]
fn precondition_ref_write_host_passes_when_clean() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    let op = Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[test]
fn precondition_checkout_rejects_host_mid_merge() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Merging;
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Host, "alpha", "refs/heads/main");
    assert!(op.check_preconditions(&vm).is_err());
}

#[test]
fn precondition_checkout_container_passes_during_host_merge() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Merging;
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Container, "alpha", "HEAD");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[test]
fn precondition_commit_rejects_unresolved_conflicts() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Conflicted;
    vm.set_repo("alpha", repo);

    let op = Op::commit("alpha", "tree123", &["ccc"], "msg");
    assert!(op.check_preconditions(&vm).is_err());
}

#[test]
fn precondition_bundle_create_requires_container_present() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_absent());

    let op = Op::bundle_create("alpha");
    assert!(op.check_preconditions(&vm).is_err());
}

#[test]
fn precondition_bundle_create_passes_with_container() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    let op = Op::bundle_create("alpha");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[test]
fn precondition_try_merge_requires_repo() {
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

#[test]
fn precondition_confirm_always_passes() {
    let vm = test_vm();
    let op = Op::confirm("proceed?");
    assert!(op.check_preconditions(&vm).is_ok());
}

#[test]
fn precondition_interactive_session_always_passes() {
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

#[test]
fn postcondition_ref_write_updates_container() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::ref_write(Side::Container, "alpha", "HEAD", "ddd");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().container, RefState::At("ddd".into()));
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}

#[test]
fn postcondition_ref_write_host_session_branch() {
    let mut vm = SyncVM::new("mysession", "main");
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::ref_write(Side::Host, "alpha", "refs/heads/mysession", "eee");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("eee".into()));
    assert_eq!(vm.repo("alpha").unwrap().container, RefState::At("aaa".into()));
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}

#[test]
fn postcondition_ref_write_host_target_branch() {
    let mut vm = SyncVM::new("mysession", "main");
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::ref_write(Side::Host, "alpha", "refs/heads/main", "fff");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("fff".into()));
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
}

#[test]
fn postcondition_bundle_fetch_updates_session() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::bundle_fetch("alpha", "/tmp/bundle");
    op.apply_postconditions(&mut vm, &OpResult::Hash("fetched123".into()));

    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("fetched123".into()));
}

#[test]
fn postcondition_checkout_clears_host_merge_state() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.host_merge_state = HostMergeState::Merging;
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Host, "alpha", "refs/heads/main");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().host_merge_state, HostMergeState::Clean);
}

#[test]
fn postcondition_checkout_container_clears_conflict() {
    let mut vm = test_vm();
    let mut repo = repo_with_refs("aaa", "bbb", "ccc");
    repo.conflict = ConflictState::Markers(vec!["file.rs".into()]);
    vm.set_repo("alpha", repo);

    let op = Op::checkout(Side::Container, "alpha", "HEAD");
    op.apply_postconditions(&mut vm, &OpResult::Unit);

    assert_eq!(vm.repo("alpha").unwrap().conflict, ConflictState::Clean);
}

#[test]
fn postcondition_commit_updates_target() {
    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let op = Op::commit("alpha", "tree123", &["ccc"], "squash merge");
    op.apply_postconditions(&mut vm, &OpResult::Hash("new_commit".into()));

    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("new_commit".into()));
    assert_eq!(vm.repo("alpha").unwrap().host_merge_state, HostMergeState::Clean);
}

#[test]
fn postcondition_agent_run_success_updates_container() {
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

#[test]
fn postcondition_agent_run_failure_keeps_conflict() {
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

#[test]
fn postcondition_interactive_session_invalidates_all_state() {
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

#[test]
fn postcondition_read_ops_dont_change_state() {
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

#[test]
fn trace_records_operations() {
    let mut vm = test_vm();
    vm.record(Op::ref_read(Side::Host, "alpha", "refs/heads/main"), OpOutcome::Ok);
    vm.record(Op::confirm("proceed?"), OpOutcome::OkWithValue("yes".into()));
    assert_eq!(vm.trace.len(), 2);
}

#[test]
fn repo_vm_empty_is_all_absent() {
    let repo = RepoVM::empty(None);
    assert_eq!(repo.container, RefState::Absent);
    assert_eq!(repo.session, RefState::Absent);
    assert_eq!(repo.target, RefState::Absent);
}

#[test]
fn repo_vm_from_refs_sets_heads() {
    let repo = repo_with_refs("aaa", "bbb", "ccc");
    assert_eq!(repo.container, RefState::At("aaa".into()));
    assert_eq!(repo.session, RefState::At("bbb".into()));
    assert_eq!(repo.target, RefState::At("ccc".into()));
}

#[test]
fn ref_state_hash_returns_value() {
    assert_eq!(RefState::At("abc".into()).hash(), Some("abc"));
    assert_eq!(RefState::Absent.hash(), None);
}

#[test]
fn sync_vm_manages_repos() {
    let mut vm = test_vm();
    assert!(vm.repo("alpha").is_none());
    vm.set_repo("alpha", repo_with_refs("a", "b", "c"));
    assert!(vm.repo("alpha").is_some());
}

// ============================================================================
// Interpreter tests — run ops through the VM with MockBackend
// ============================================================================

#[test]
fn interpreter_runs_primitive_sequence() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::BundleCreate, MockResult::Hash("/tmp/alpha.bundle".into()));
    mock.on(CallMatcher::BundleFetch, MockResult::Hash("fetched_abc".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "old_session", "ccc"));

    let result = vm.run(&mock, vec![
        Op::bundle_create("alpha"),
        Op::bundle_fetch("alpha", "/tmp/alpha.bundle"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/test-session", "fetched_abc"),
    ]);

    assert_eq!(result.succeeded(), 3);
    assert!(!result.halted);
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("fetched_abc".into()));

    let calls = mock.calls();
    assert_eq!(calls.len(), 3);
    assert!(matches!(calls[0], RecordedCall::BundleCreate { .. }));
    assert!(matches!(calls[1], RecordedCall::BundleFetch { .. }));
    assert!(matches!(calls[2], RecordedCall::RefWrite { .. }));
}

#[test]
fn interpreter_halts_on_precondition_failure() {
    let mock = MockBackend::new();
    let mut vm = test_vm();

    let result = vm.run(&mock, vec![
        Op::bundle_create("alpha"),
    ]);

    assert!(result.halted);
    assert_eq!(result.succeeded(), 0);
    assert!(mock.calls().is_empty());
}

#[test]
fn interpreter_halts_on_backend_error() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::BundleCreate, MockResult::Error("disk full".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::bundle_create("alpha"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd"),
    ]);

    assert!(result.halted);
    assert_eq!(result.outcomes.len(), 1);
    assert_eq!(vm.repo("alpha").unwrap().session, RefState::At("bbb".into()));
}

#[test]
fn interpreter_confirm_decline_halts() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::PromptUser, MockResult::Bool(false));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::confirm("proceed?"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd"),
    ]);

    assert!(result.halted);
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}

#[test]
fn interpreter_confirm_accept_continues() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::PromptUser, MockResult::Bool(true));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::confirm("proceed?"),
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "ddd"),
    ]);

    assert!(!result.halted);
    assert_eq!(result.succeeded(), 2);
}

#[test]
fn interpreter_try_merge_follows_clean_path() {
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
    ]);

    assert!(!result.halted);
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("new_commit".into()));
    assert!(!mock.was_called(&CallMatcher::PromptUser));
    assert!(mock.was_called(&CallMatcher::Commit));
}

#[test]
fn interpreter_try_merge_follows_conflict_path() {
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
    ]);

    assert!(!result.halted);
    assert!(mock.was_called(&CallMatcher::AgentRun));
    assert!(mock.was_called(&CallMatcher::BundleCreate));
    assert!(!mock.was_called(&CallMatcher::Commit));
    assert_eq!(vm.repo("gamma").unwrap().conflict, ConflictState::Resolved);
}

#[test]
fn interpreter_agent_failure_follows_failure_path() {
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
    ]);

    assert!(!result.halted);
    assert!(!mock.was_called(&CallMatcher::BundleCreate));
    assert!(mock.was_called(&CallMatcher::Checkout));
    assert_eq!(vm.repo("alpha").unwrap().conflict, ConflictState::Clean);
}

#[test]
fn interpreter_interactive_session_invalidates_state() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::InteractiveSession, MockResult::ContainerExited(0));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));
    vm.set_repo("beta", repo_with_refs("ddd", "eee", "fff"));

    let result = vm.run(&mock, vec![
        Op::InteractiveSession { prompt: Some("hello".into()), on_exit: vec![] },
    ]);

    assert!(!result.halted);
    assert_eq!(vm.repo("alpha").unwrap().container, RefState::Absent);
    assert_eq!(vm.repo("beta").unwrap().container, RefState::Absent);
}

#[test]
fn interpreter_state_unchanged_on_backend_error() {
    let mock = MockBackend::new();
    mock.on(CallMatcher::RefWrite, MockResult::Error("permission denied".into()));

    let mut vm = test_vm();
    vm.set_repo("alpha", repo_with_refs("aaa", "bbb", "ccc"));

    let result = vm.run(&mock, vec![
        Op::ref_write(Side::Host, "alpha", "refs/heads/main", "new_hash"),
    ]);

    assert!(result.halted);
    assert_eq!(vm.repo("alpha").unwrap().target, RefState::At("ccc".into()));
}
