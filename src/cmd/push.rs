use crate::types::*;
use crate::render;
use git_sandbox::vm::{self, programs::plan_push, RealBackend, display::render_program};
use colored::Colorize;

use super::confirm;
use super::sync_cmd::{build_sync_plan, build_vm_from_plan};

pub(crate) async fn cmd_push(name: &SessionName, branch: &str, filter: Option<&str>, include_deps: bool, dry_run: bool, auto_yes: bool, force: bool) -> anyhow::Result<()> {
    let (lc, engine, plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    // VM planning
    let mut vm = build_vm_from_plan(name, branch, &plan.action, &repo_paths);
    let mut push_ops = plan_push(&vm);

    // Handle --force: add ForceInject for blocked repos
    if force {
        for action in &plan.action.repo_actions {
            if matches!(action.state.push_action(), PushAction::Blocked(_)) {
                push_ops.push(vm::Op::ForceInject {
                    repo: action.repo_name.clone(),
                    branch: branch.to_string(),
                });
                push_ops.push(vm::Op::Extract {
                    repo: action.repo_name.clone(),
                    session_branch: name.to_string(),
                });
            }
        }
    }

    let has_pushes = !push_ops.is_empty();

    // Preview
    render::sync_plan_directed(&plan.action, "push");

    if force && plan.action.repo_actions.iter().any(|a| matches!(a.state.push_action(), PushAction::Blocked(_))) {
        let blocked_count = plan.action.repo_actions.iter()
            .filter(|a| matches!(a.state.push_action(), PushAction::Blocked(_)))
            .count();
        eprintln!("  {} --force: {} repo(s) will be hard-reset to match {}", "⚠".yellow(), blocked_count, branch);
    }

    if dry_run {
        if !push_ops.is_empty() {
            eprintln!("\nProgram ({} ops):", push_ops.len());
            eprint!("{}", render_program(&push_ops, 2));
        }
        return Ok(());
    }

    if !has_pushes {
        return Ok(());
    }

    if !confirm("\n  Execute push?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    // Execute via VM interpreter with RealBackend
    eprintln!();
    let backend = RealBackend::from_docker(lc.docker_client().clone(), name.as_str());
    let result = vm.run(&backend, push_ops).await;

    // Render results
    let succeeded = result.succeeded();
    let failed = result.failed();

    if result.halted {
        eprintln!("  {} Push halted: {}", "✗".red(), result.halt_reason.as_deref().unwrap_or("unknown"));
    }

    for outcome in &result.outcomes {
        if outcome.result.is_ok() {
            eprintln!("  {} {}", "✓".green(), outcome.op_description);
        } else {
            match &outcome.result {
                vm::StepResult::BackendError(e) => {
                    eprintln!("  {} {} — {}", "✗".red(), outcome.op_description, e);
                }
                vm::StepResult::PreconditionFailed(reason) => {
                    eprintln!("  {} {} — precondition: {}", "✗".red(), outcome.op_description, reason);
                }
                _ => {
                    eprintln!("  {} {}", "✗".red(), outcome.op_description);
                }
            }
        }
    }

    eprintln!();
    if failed == 0 {
        eprintln!("  {} {} op(s) succeeded", "✓".green(), succeeded);
    } else {
        eprintln!("  {} {} succeeded, {} failed", "⚠".yellow(), succeeded, failed);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires Docker
    async fn cmd_push_is_callable() {
        let name = SessionName::new("test-nonexistent-push");
        let result = cmd_push(&name, "main", None, false, true, true, false).await;
        assert!(result.is_err());
    }
}
