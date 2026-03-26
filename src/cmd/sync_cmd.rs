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

    let has_work = plan.action.has_work();

    render::sync_plan_directed(&plan.action, "sync");

    if dry_run || !has_work {
        return Ok(());
    }

    if !confirm("\n  Execute sync?", auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    eprintln!();
    let result = engine.execute_sync(name, plan.action, &repo_paths).await?;
    render::sync_result(&result);

    let conflicts = collect_conflicts(&result, &repo_paths);
    if !conflicts.is_empty() {
        offer_reconciliation(&lc, name, &conflicts, branch).await?;
    }

    Ok(())
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
