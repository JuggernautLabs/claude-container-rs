//! Plan visualization tests — verify and display what plans look like
//! for every meaningful repo state.
//!
//! These are both tests AND documentation. Each test prints the plan
//! so you can see exactly what ops would execute.

use gitvm::vm::*;
use std::path::PathBuf;

fn vm_with_repo(container: &str, session: &str, target: &str) -> SyncVM {
    let mut vm = SyncVM::new("session", "main");
    vm.set_repo("repo", RepoVM::from_refs(
        if container.is_empty() { RefState::Absent } else { RefState::At(container.into()) },
        if session.is_empty() { RefState::Absent } else { RefState::At(session.into()) },
        if target.is_empty() { RefState::Absent } else { RefState::At(target.into()) },
        Some(PathBuf::from("/host/repo")),
    ));
    vm
}

fn show_plan(label: &str, vm: &SyncVM) {
    let push_ops = programs::plan_push(vm);
    let pull_ops = programs::plan_pull(vm);
    let sync_ops = programs::plan_sync(vm);

    eprintln!("\n  ╔══ {} ══╗", label);

    // Show repo state
    for (name, repo) in &vm.repos {
        eprintln!("  │ {}: container={} session={} target={}{}{}",
            name,
            match &repo.container { RefState::At(h) => &h[..7.min(h.len())], RefState::Absent => "absent", RefState::Stale => "stale" },
            match &repo.session { RefState::At(h) => &h[..7.min(h.len())], RefState::Absent => "absent", RefState::Stale => "stale" },
            match &repo.target { RefState::At(h) => &h[..7.min(h.len())], RefState::Absent => "absent", RefState::Stale => "stale" },
            if !repo.container_clean { " [container dirty]" } else { "" },
            if !repo.host_clean { " [host dirty]" } else { "" },
        );
    }

    eprintln!("  ├── push: {}", if push_ops.is_empty() { "(nothing to push)".into() } else { format!("{} op(s)", push_ops.len()) });
    if !push_ops.is_empty() { eprint!("{}", render_program(&push_ops, 6)); }

    eprintln!("  ├── pull: {}", if pull_ops.is_empty() { "(nothing to pull)".into() } else { format!("{} op(s)", pull_ops.len()) });
    if !pull_ops.is_empty() { eprint!("{}", render_program(&pull_ops, 6)); }

    eprintln!("  ├── sync: {}", if sync_ops.is_empty() { "(nothing to sync)".into() } else { format!("{} op(s)", sync_ops.len()) });
    if !sync_ops.is_empty() { eprint!("{}", render_program(&sync_ops, 6)); }

    eprintln!("  ╚══════════════════════════════╝");
    eprintln!("  sync: {}", if sync_ops.is_empty() { "(empty)".into() } else { format!("{} ops", sync_ops.len()) });
    for op in &sync_ops { eprintln!("    {}", op); }
}

// ============================================================================
// All states → plans
// ============================================================================

#[test]
fn plan_for_all_in_sync() {
    let vm = vm_with_repo("abc", "abc", "abc");
    show_plan("ALL IN SYNC (container=session=target)", &vm);

    assert!(programs::plan_push(&vm).is_empty());
    assert!(programs::plan_pull(&vm).is_empty());
    assert!(programs::plan_sync(&vm).is_empty());
}

#[test]
fn plan_for_container_ahead() {
    // Agent made commits in container, not yet extracted
    let vm = vm_with_repo("new_work", "old", "old");
    show_plan("CONTAINER AHEAD (agent worked, not extracted)", &vm);

    let push = programs::plan_push(&vm);
    let pull = programs::plan_pull(&vm);

    // Push: container != session, session != target? No — session == target.
    // So push has nothing (target didn't move).
    assert!(push.is_empty(), "push should be empty when only container ahead");

    // Pull: Extract (get agent's work to host)
    assert!(pull.iter().any(|op| matches!(op, Op::Extract { .. })),
        "pull should Extract when container ahead");
}

#[test]
fn plan_for_target_ahead() {
    // Someone pushed to main while agent was idle
    let vm = vm_with_repo("same", "same", "moved");
    show_plan("TARGET AHEAD (someone pushed to main)", &vm);

    let push = programs::plan_push(&vm);
    let pull = programs::plan_pull(&vm);

    // Push: target moved → Inject
    assert!(push.iter().any(|op| matches!(op, Op::Inject { .. })),
        "push should Inject when target ahead");

    // Pull: session == container, but session != target → MergeToTarget
    assert!(pull.iter().any(|op| matches!(op, Op::TryMerge { .. })),
        "pull should MergeToTarget when target ahead");
}

#[test]
fn plan_for_both_diverged() {
    // Agent worked AND someone pushed to main
    let vm = vm_with_repo("agent_new", "old_session", "external_new");
    show_plan("DIVERGED (agent worked + external push to main)", &vm);

    let pull = programs::plan_pull(&vm);

    // Should be Reconcile: inject + extract + merge
    assert!(pull.iter().any(|op| matches!(op, Op::Inject { .. })),
        "reconcile should Inject target into container");
    assert!(pull.iter().any(|op| matches!(op, Op::Extract { .. })),
        "reconcile should Extract after inject");
    assert!(pull.iter().any(|op| matches!(op, Op::TryMerge { .. })),
        "reconcile should Merge into target");
}

#[test]
fn plan_for_container_only_no_session() {
    // First-time repo — container has it, host doesn't
    let vm = vm_with_repo("abc", "", "");
    show_plan("CONTAINER ONLY (no session branch, no target)", &vm);

    let pull = programs::plan_pull(&vm);
    assert!(pull.iter().any(|op| matches!(op, Op::Extract { .. })),
        "should Extract (clone to host)");
}

#[test]
fn plan_for_host_only_no_container() {
    // Repo on host but not in container
    let vm = vm_with_repo("", "abc", "abc");
    show_plan("HOST ONLY (no container)", &vm);

    let push = programs::plan_push(&vm);
    let pull = programs::plan_pull(&vm);

    // Push: absent container → Clone
    assert!(push.iter().any(|op| matches!(op, Op::RunContainer { .. })),
        "should clone into container");

    // Pull: nothing to pull (no container)
    assert!(pull.is_empty(), "nothing to pull from absent container");
}

#[test]
fn plan_for_session_ahead_of_target() {
    // Extract already happened, session has work target doesn't
    let vm = vm_with_repo("new", "new", "old");
    show_plan("SESSION AHEAD OF TARGET (extracted, needs merge)", &vm);

    let pull = programs::plan_pull(&vm);
    assert!(pull.iter().any(|op| matches!(op, Op::TryMerge { .. })),
        "should merge session into target");

    let push = programs::plan_push(&vm);
    // VM can't distinguish "target behind" from "target ahead" without
    // ancestry. Push fires (Inject), but the inject will be a no-op
    // inside the container (target is ancestor of container HEAD).
    // This is safe — redundant but not incorrect.
    assert!(push.iter().any(|op| matches!(op, Op::Inject { .. })),
        "push fires when target differs (even if behind) — inject is no-op in container");
}

#[test]
fn plan_for_dirty_host() {
    let mut vm = SyncVM::new("session", "main");
    let mut repo = RepoVM::from_refs(
        RefState::At("new".into()),
        RefState::At("old".into()),
        RefState::At("old".into()),
        Some(PathBuf::from("/host/repo")),
    );
    repo.host_clean = false;
    vm.set_repo("repo", repo);
    show_plan("HOST DIRTY (uncommitted changes)", &vm);

    let pull = programs::plan_pull(&vm);
    assert!(pull.is_empty(), "pull should be blocked when host dirty");

    let push = programs::plan_push(&vm);
    // Push doesn't care about host dirty
    // But in this case target == session, so no push work anyway
    assert!(push.is_empty(), "no push work when target hasn't moved");
}

#[test]
fn plan_for_dirty_container() {
    let mut vm = SyncVM::new("session", "main");
    let mut repo = RepoVM::from_refs(
        RefState::At("same".into()),
        RefState::At("same".into()),
        RefState::At("ahead".into()),
        Some(PathBuf::from("/host/repo")),
    );
    repo.container_clean = false;
    vm.set_repo("repo", repo);
    show_plan("CONTAINER DIRTY + TARGET AHEAD", &vm);

    let push = programs::plan_push(&vm);
    // Container dirty blocks push (can't merge into dirty container)
    assert!(push.is_empty(), "push should be blocked when container dirty");
}

#[test]
fn plan_for_everything_absent() {
    let vm = vm_with_repo("", "", "");
    show_plan("ALL ABSENT", &vm);

    assert!(programs::plan_push(&vm).is_empty());
    assert!(programs::plan_pull(&vm).is_empty());
    assert!(programs::plan_sync(&vm).is_empty());
}

// ============================================================================
// Multi-repo plans
// ============================================================================

#[test]
fn plan_for_mixed_multi_repo() {
    let mut vm = SyncVM::new("session", "main");

    // alpha: container ahead (needs pull)
    vm.set_repo("alpha", RepoVM::from_refs(
        RefState::At("alpha_new".into()),
        RefState::At("alpha_old".into()),
        RefState::At("alpha_old".into()),
        Some(PathBuf::from("/host/alpha")),
    ));

    // beta: all in sync (skip)
    vm.set_repo("beta", RepoVM::from_refs(
        RefState::At("same".into()),
        RefState::At("same".into()),
        RefState::At("same".into()),
        Some(PathBuf::from("/host/beta")),
    ));

    // gamma: target ahead (needs push)
    vm.set_repo("gamma", RepoVM::from_refs(
        RefState::At("g_same".into()),
        RefState::At("g_same".into()),
        RefState::At("g_ahead".into()),
        Some(PathBuf::from("/host/gamma")),
    ));

    // delta: both diverged (reconcile)
    vm.set_repo("delta", RepoVM::from_refs(
        RefState::At("d_container".into()),
        RefState::At("d_old".into()),
        RefState::At("d_target".into()),
        Some(PathBuf::from("/host/delta")),
    ));

    show_plan("MULTI-REPO: alpha=pull, beta=skip, gamma=push, delta=reconcile", &vm);

    let sync_ops = programs::plan_sync(&vm);

    // Verify each repo gets correct treatment
    let has = |name: &str, op_type: &str| -> bool {
        sync_ops.iter().any(|op| {
            let op_str = format!("{}", op);
            op_str.contains(name) && match op_type {
                "inject" => matches!(op, Op::Inject { .. }),
                "extract" => matches!(op, Op::Extract { .. }),
                "merge" => matches!(op, Op::TryMerge { .. }),
                _ => false,
            }
        })
    };

    assert!(has("alpha", "extract"), "alpha should be extracted");
    assert!(!has("beta", "extract") && !has("beta", "inject"), "beta should have no ops");
    assert!(has("gamma", "inject"), "gamma should be injected");
    assert!(has("delta", "inject"), "delta (reconcile) should inject");
    assert!(has("delta", "extract"), "delta (reconcile) should extract");
}
