use crate::types::*;
use crate::render;
use git_sandbox::vm::programs::plan_push;
use colored::Colorize;

use super::confirm;
use super::sync_cmd::{build_sync_plan, build_vm_from_plan};

pub(crate) async fn cmd_push(name: &SessionName, branch: &str, filter: Option<&str>, include_deps: bool, dry_run: bool, auto_yes: bool, force: bool) -> anyhow::Result<()> {
    let (_lc, engine, plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    // VM planning: determine what needs pushing
    let vm = build_vm_from_plan(name, branch, &plan.action, &repo_paths);
    let push_ops = plan_push(&vm);
    let has_pushes = !push_ops.is_empty();

    let has_force_targets = force && plan.action.repo_actions.iter()
        .any(|a| matches!(a.state.push_action(), PushAction::Blocked(_)));

    render::sync_plan_directed(&plan.action, "push");

    if force && has_force_targets {
        let blocked: Vec<_> = plan.action.repo_actions.iter()
            .filter(|a| matches!(a.state.push_action(), PushAction::Blocked(_)))
            .collect();
        eprintln!("  {} --force: {} repo(s) will be hard-reset to match {}", "⚠".yellow(), blocked.len(), branch);
    }

    if dry_run || (!has_pushes && !has_force_targets) {
        return Ok(());
    }

    if !confirm("\n  Execute push?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    eprintln!();
    let result = engine.execute_push_with_force(name, plan.action, &repo_paths, force).await?;
    render::sync_result(&result);
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
