//! Compound programs — build op trees from high-level intents.
//!
//! Each function returns an Op (or Vec<Op>) that decomposes a
//! high-level operation into primitives. The interpreter walks
//! the tree recursively.

use super::ops::*;
use super::state::*;

// ============================================================================
// Compound builders — one function per high-level operation
// ============================================================================

/// Extract: move container HEAD to host session branch.
/// bundle_create → bundle_fetch → ref_write(session)
pub fn ops_extract(repo: &str, session_name: &str) -> Vec<Op> {
    vec![
        Op::BundleCreate { repo: repo.into() },
        Op::BundleFetch { repo: repo.into(), bundle_path: "BUNDLE".into() },
        Op::ref_write(
            Side::Host, repo,
            &format!("refs/heads/{}", session_name),
            "FETCHED",
        ),
    ]
}

/// Inject: push host target branch into container via throwaway container.
pub fn ops_inject(repo: &str, branch: &str) -> Vec<Op> {
    let script = format!(
        r#"git config --global --add safe.directory "*"
cd "/session/{repo}" || exit 1
git remote add _cc_upstream "/upstream" 2>/dev/null || git remote set-url _cc_upstream "/upstream"
git fetch _cc_upstream "{branch}" || exit 1
if ! git merge "_cc_upstream/{branch}" --no-edit 2>&1; then
    git merge --abort 2>/dev/null || true
    git remote remove _cc_upstream 2>/dev/null
    exit 1
fi
git remote remove _cc_upstream 2>/dev/null"#,
        repo = repo, branch = branch,
    );
    vec![
        Op::RunContainer {
            image: "alpine/git".into(),
            script,
            mounts: vec![],  // filled by interpreter from VM state
        },
    ]
}

/// Merge: session branch → target branch on host.
/// checkout(target) → merge_trees → commit → ref_write(target)
/// Uses TryMerge for conflict handling.
pub fn ops_merge(
    repo: &str,
    session_head: &str,
    target_head: &str,
    target_branch: &str,
    squash: bool,
) -> Op {
    let msg = if squash {
        format!("squash: session into {}", target_branch)
    } else {
        format!("merge: session into {}", target_branch)
    };

    Op::TryMerge {
        repo: repo.into(),
        ours: target_head.into(),
        theirs: session_head.into(),
        on_clean: vec![
            Op::checkout(Side::Host, repo, &format!("refs/heads/{}", target_branch)),
            Op::commit(repo, "MERGED_TREE", &[target_head], &msg),
            Op::ref_write(
                Side::Host, repo,
                &format!("refs/heads/{}", target_branch),
                "NEW_COMMIT",
            ),
        ],
        on_conflict: vec![],  // caller decides: agent or skip
        on_error: vec![
            Op::checkout(Side::Host, repo, &format!("refs/heads/{}", target_branch)),
        ],
    }
}

/// Clone: first-time clone from host into container volume.
pub fn ops_clone(repo: &str) -> Vec<Op> {
    let script = format!(
        r#"export HOME=/tmp
git config --global --add safe.directory "*"
[ -d "/session/{repo}" ] && rm -rf "/session/{repo}"
git clone "/upstream" "/session/{repo}" || {{ rm -rf "/session/{repo}"; exit 1; }}
chown -R $(id -u):$(id -g) "/session/{repo}" 2>/dev/null || true"#,
        repo = repo,
    );
    vec![
        Op::RunContainer {
            image: "alpine/git".into(),
            script,
            mounts: vec![],
        },
    ]
}

/// Reconcile (clean): inject target into container, then extract + merge.
pub fn ops_reconcile_clean(
    repo: &str,
    session_name: &str,
    session_head: &str,
    target_head: &str,
    target_branch: &str,
) -> Vec<Op> {
    vec![
        Op::Inject { repo: repo.into(), branch: target_branch.into() },
        Op::Extract { repo: repo.into(), session_branch: session_name.into() },
        ops_merge(repo, session_head, target_head, target_branch, true),
    ]
}

/// Reconcile (conflicted): merge into volume → agent → extract + merge.
pub fn ops_reconcile_with_agent(
    repo: &str,
    session_name: &str,
    target_head: &str,
    target_branch: &str,
    conflict_files: Vec<String>,
) -> Vec<Op> {
    vec![
        Op::RunContainer {
            image: "alpine/git".into(),
            script: format!(
                r#"git config --global --add safe.directory "*"
cd "/session/{repo}" || exit 1
git fetch "/upstream" "{branch}" || exit 1
git merge FETCH_HEAD --no-commit || true"#,
                repo = repo, branch = target_branch,
            ),
            mounts: vec![],
        },
        Op::AgentRun {
            repo: repo.into(),
            task: AgentTask::ResolveConflicts { files: conflict_files },
            context: String::new(),
            on_success: vec![
                Op::Extract { repo: repo.into(), session_branch: session_name.into() },
                ops_merge(repo, "RESOLVED_HEAD", target_head, target_branch, true),
            ],
            on_failure: vec![
                Op::checkout(Side::Container, repo, "HEAD"),
            ],
        },
    ]
}

// ============================================================================
// Program generators — read VM state, emit programs
// ============================================================================

/// Generate a push program: inject repos where target is ahead of container.
pub fn plan_push(vm: &SyncVM) -> Vec<Op> {
    let mut ops = Vec::new();
    for (name, repo) in &vm.repos {
        let push = repo_push_action(repo, &vm.target_branch);
        match push {
            PushIntent::Inject => {
                ops.push(Op::Inject { repo: name.clone(), branch: vm.target_branch.clone() });
                // Re-extract after inject to keep session in sync
                ops.push(Op::Extract { repo: name.clone(), session_branch: vm.session_name.clone() });
            }
            PushIntent::Clone => {
                ops.extend(ops_clone(name));
            }
            PushIntent::Skip | PushIntent::Blocked(_) => {}
        }
    }
    ops
}

/// Generate a pull program: extract + merge for repos where container
/// is ahead, or just merge for repos where session is ahead of target.
pub fn plan_pull(vm: &SyncVM) -> Vec<Op> {
    let mut extract_ops = Vec::new();
    let mut merge_ops = Vec::new();

    for (name, repo) in &vm.repos {
        let pull = repo_pull_action(repo);
        match pull {
            PullIntent::Extract => {
                extract_ops.push(Op::Extract { repo: name.clone(), session_branch: vm.session_name.clone() });
                if let (RefState::At(s), RefState::At(t)) = (&repo.session, &repo.target) {
                    merge_ops.push(ops_merge(name, s, t, &vm.target_branch, true));
                }
            }
            PullIntent::MergeToTarget => {
                if let (RefState::At(s), RefState::At(t)) = (&repo.session, &repo.target) {
                    merge_ops.push(ops_merge(name, s, t, &vm.target_branch, true));
                }
            }
            PullIntent::CloneToHost => {
                extract_ops.push(Op::Extract { repo: name.clone(), session_branch: vm.session_name.clone() });
            }
            PullIntent::Reconcile { has_conflicts, conflict_files } => {
                if has_conflicts {
                    if let RefState::At(t) = &repo.target {
                        extract_ops.extend(ops_reconcile_with_agent(
                            name, &vm.session_name, t, &vm.target_branch, conflict_files,
                        ));
                    }
                } else {
                    if let (RefState::At(s), RefState::At(t)) = (&repo.session, &repo.target) {
                        extract_ops.extend(ops_reconcile_clean(
                            name, &vm.session_name, s, t, &vm.target_branch,
                        ));
                    }
                }
            }
            PullIntent::Skip | PullIntent::Blocked(_) => {}
        }
    }

    let mut ops = extract_ops;
    if !merge_ops.is_empty() {
        ops.extend(merge_ops);
    }
    ops
}

/// Generate a sync program: push first, then pull.
pub fn plan_sync(vm: &SyncVM) -> Vec<Op> {
    let mut ops = plan_push(vm);
    ops.extend(plan_pull(vm));
    ops
}

// ============================================================================
// Intent types — what a repo needs (derived from RepoVM state)
// ============================================================================

/// What push should do for one repo.
#[derive(Debug, Clone, PartialEq)]
pub enum PushIntent {
    Skip,
    Inject,
    Clone,
    Blocked(String),
}

/// What pull should do for one repo.
#[derive(Debug, Clone, PartialEq)]
pub enum PullIntent {
    Skip,
    Extract,
    CloneToHost,
    MergeToTarget,
    Reconcile { has_conflicts: bool, conflict_files: Vec<String> },
    Blocked(String),
}

/// Derive push intent from repo state.
pub fn repo_push_action(repo: &RepoVM, target_branch: &str) -> PushIntent {
    // Container-side blockers block push
    if !repo.container_clean {
        return PushIntent::Blocked("container dirty".into());
    }
    if repo.host_merge_state != HostMergeState::Clean {
        // Host merge state doesn't block push (we read a ref, not worktree)
    }

    match (&repo.container, &repo.session, &repo.target) {
        // No container → push to container (clone)
        (RefState::Absent, _, RefState::At(_)) => PushIntent::Clone,
        // Target ahead of session → inject
        (_, RefState::At(s), RefState::At(t)) if s != t => {
            // Simplified: if target differs from session, inject
            // Real logic would check ancestry
            PushIntent::Inject
        }
        _ => PushIntent::Skip,
    }
}

/// Derive pull intent from repo state.
pub fn repo_pull_action(repo: &RepoVM) -> PullIntent {
    if !repo.host_clean {
        return PullIntent::Blocked("host dirty".into());
    }

    match (&repo.container, &repo.session, &repo.target) {
        // No container — nothing to extract
        (RefState::Absent, _, _) => PullIntent::Skip,
        // Container has work session doesn't
        (RefState::At(c), RefState::At(s), _) if c != s => PullIntent::Extract,
        (RefState::At(_), RefState::Absent, _) => PullIntent::CloneToHost,
        // Session ahead of target
        (RefState::At(c), RefState::At(s), RefState::At(t)) if s != t => {
            // Check if container matches session (extraction done)
            if c == s {
                PullIntent::MergeToTarget
            } else {
                PullIntent::Extract
            }
        }
        _ => PullIntent::Skip,
    }
}
