use crate::types::*;
use crate::lifecycle;
use crate::session;
use crate::sync;
use crate::render;
use colored::Colorize;

use super::confirm;
use super::pull::{collect_conflicts, offer_reconciliation};

pub(crate) async fn cmd_sync_preview(name: &SessionName, branch: &str, filter: Option<&str>) -> anyhow::Result<()> {
    let (_lc, _engine, plan, _repo_paths) = build_sync_plan(name, branch, filter, false).await?;
    render::sync_plan_directed(&plan.action, "status");
    Ok(())
}

pub(crate) async fn cmd_sync(name: &SessionName, branch: &str, filter: Option<&str>, include_deps: bool, dry_run: bool, auto_yes: bool) -> anyhow::Result<()> {
    let (lc, engine, plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    // VM planning
    let mut vm = build_vm_from_plan(name, branch, &plan.action, &repo_paths);
    let sync_ops = gitvm::vm::programs::plan_sync(&vm);
    let has_work = !sync_ops.is_empty();

    render::sync_plan_directed(&plan.action, "sync");

    if dry_run {
        if !sync_ops.is_empty() {
            eprintln!("\nProgram ({} ops):", sync_ops.len());
            eprint!("{}", gitvm::vm::display::render_program(&sync_ops, 2));
        }
        return Ok(());
    }

    if !has_work {
        return Ok(());
    }

    if !confirm("\n  Execute sync?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    // Execute via VM interpreter
    eprintln!();
    let backend = gitvm::vm::RealBackend::from_docker(lc.docker_client().clone(), name.as_str());
    let result = vm.run(&backend, sync_ops).await;

    // Render results
    for outcome in &result.outcomes {
        if outcome.result.is_ok() {
            eprintln!("  {} {}", colored::Colorize::green("✓"), outcome.op_description);
        } else {
            match &outcome.result {
                gitvm::vm::StepResult::BackendError(e) => {
                    eprintln!("  {} {} — {}", colored::Colorize::red("✗"), outcome.op_description, e);
                }
                _ => {
                    eprintln!("  {} {}", colored::Colorize::red("✗"), outcome.op_description);
                }
            }
        }
    }

    if result.halted {
        eprintln!("  {} Sync halted: {}", colored::Colorize::red("✗"),
            result.halt_reason.as_deref().unwrap_or("unknown"));
    }

    let succeeded = result.succeeded();
    let failed = result.failed();
    eprintln!();
    if failed == 0 {
        eprintln!("  {} {} op(s) succeeded", colored::Colorize::green("✓"), succeeded);
    } else {
        eprintln!("  {} {} succeeded, {} failed", colored::Colorize::yellow("⚠"), succeeded, failed);
    }

    Ok(())
}

/// Build a VM from a sync plan (shared helper).
pub(crate) fn build_vm_from_plan(
    name: &SessionName,
    branch: &str,
    plan: &SessionSyncPlan,
    repo_paths: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> gitvm::vm::SyncVM {
    use gitvm::vm::{SyncVM, RepoVM, RefState};

    let mut vm = SyncVM::new(name.as_str(), branch);
    for action in &plan.repo_actions {
        let host_path = repo_paths.get(&action.repo_name).cloned();
        vm.set_repo(&action.repo_name, RepoVM::from_refs(
            action.container_head.as_ref().map(|h| RefState::At(h.to_string())).unwrap_or(RefState::Absent),
            action.session_head.as_ref().map(|h| RefState::At(h.to_string())).unwrap_or(RefState::Absent),
            action.target_head.as_ref().map(|h| RefState::At(h.to_string())).unwrap_or(RefState::Absent),
            host_path,
        ));
    }
    vm
}

/// Shared: build a sync plan (used by pull, push, sync, status)
pub(crate) async fn build_sync_plan(
    name: &SessionName,
    branch: &str,
    filter: Option<&str>,
    include_deps: bool,
) -> anyhow::Result<(
    lifecycle::Lifecycle,
    sync::SyncEngine,
    Plan<SessionSyncPlan>,
    std::collections::BTreeMap<String, std::path::PathBuf>,
)> {
    let lc = lifecycle::Lifecycle::new()?;
    lc.ensure_util_image().await;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    let config = sm.read_or_discover_config(name).await?;

    let mut repo_paths: std::collections::BTreeMap<String, std::path::PathBuf> = config.projects.iter()
        .filter(|(_, pcfg)| include_deps || pcfg.role == config::RepoRole::Project)
        .map(|(pname, pcfg)| (pname.clone(), pcfg.path.clone()))
        .collect();

    // Apply regex filter if provided
    if let Some(pattern) = filter {
        let re = regex::Regex::new(pattern)
            .map_err(|e| anyhow::anyhow!("Invalid filter regex '{}': {}", pattern, e))?;
        repo_paths.retain(|name, _| re.is_match(name));
        if repo_paths.is_empty() {
            anyhow::bail!("No repos match filter '{}'", pattern);
        }
    }

    let engine = sync::SyncEngine::new(lc.docker_client().clone());
    let plan = engine.plan_sync(name, branch, &repo_paths).await?;

    Ok((lc, engine, plan, repo_paths))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // requires Docker
    async fn build_sync_plan_is_callable() {
        let name = SessionName::new("test-nonexistent-plan");
        let result = build_sync_plan(&name, "main", None, false).await;
        // Will fail because session doesn't exist, but verifies the function signature
        assert!(result.is_err());
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn cmd_sync_preview_is_callable() {
        let name = SessionName::new("test-nonexistent-preview");
        let _ = cmd_sync_preview(&name, "main", None).await;
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn cmd_sync_dry_run() {
        let name = SessionName::new("test-nonexistent-sync");
        let result = cmd_sync(&name, "main", None, false, true, true).await;
        assert!(result.is_err());
    }
}
