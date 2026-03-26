use crate::types::*;
use crate::lifecycle;
use crate::session;
use crate::sync;
use crate::container;
use crate::render;
use crate::scripts;
use std::path::PathBuf;
use colored::Colorize;

use super::confirm;
use super::sync_cmd::build_sync_plan;

/// Extract-only: pull container work into session branches, no merge into target.
/// Shows a diff preview of what changed in the container vs the host.
pub(crate) async fn cmd_extract(name: &SessionName, filter: Option<&str>, dry_run: bool, auto_yes: bool) -> anyhow::Result<()> {
    let lc = lifecycle::Lifecycle::new()?;
    lc.ensure_util_image().await;
    let sm = session::SessionManager::new(lc.docker_client().clone());

    let config = sm.read_or_discover_config(name).await?;

    let engine = sync::SyncEngine::new(lc.docker_client().clone());

    // Snapshot container
    let mut volume_repos = engine.snapshot(name, "").await?;

    // Apply filter
    if let Some(pattern) = filter {
        let re = regex::Regex::new(pattern)
            .map_err(|e| anyhow::anyhow!("Invalid filter regex '{}': {}", pattern, e))?;
        volume_repos.retain(|vr| re.is_match(&vr.name));
        if volume_repos.is_empty() {
            anyhow::bail!("No repos match filter '{}'", pattern);
        }
    }

    // Classify: new (no session branch on host) vs changed (container ahead of session branch)
    let mut changed = Vec::new();
    let mut unchanged = 0u32;

    for vr in &volume_repos {
        let host_path = match config.projects.get(&vr.name) {
            Some(cfg) => &cfg.path,
            None => continue,
        };
        let session_branch = name.to_string();

        // Check if session branch exists on host and compare
        let host_session_head = git2::Repository::open(host_path).ok()
            .and_then(|repo| {
                repo.find_reference(&format!("refs/heads/{}", session_branch)).ok()
                    .and_then(|r| r.peel_to_commit().ok())
                    .map(|c| CommitHash::new(c.id().to_string()))
            });

        let container_head = &vr.head;

        // Compute diff: host session branch HEAD → container HEAD
        let diff = host_session_head.as_ref().and_then(|h_head| {
            engine.compute_diff(host_path, h_head, container_head)
        });

        let is_same = host_session_head.as_ref().map_or(false, |h| h.as_str() == container_head.as_str());
        if is_same {
            unchanged += 1;
            continue;
        }

        let status = if host_session_head.is_none() { "new" } else { "changed" };
        changed.push((vr, host_path.clone(), session_branch, diff, status));
    }

    // Render preview
    render::rule(Some(&format!("extract: {}", name)));
    if changed.is_empty() {
        eprintln!("{}", "Nothing new to extract.".dimmed());
        return Ok(());
    }

    eprintln!("{} to extract, {} unchanged", changed.len(), unchanged);
    eprintln!();

    for (vr, host_path, session_branch, _, status) in &changed {
        let short_head = &vr.head.as_str()[..7.min(vr.head.as_str().len())];
        let size_str = if vr.git_size_mb > 0 {
            format!(" {}MB", vr.git_size_mb)
        } else {
            String::new()
        };
        if *status == "new" {
            eprintln!("  {} {} → {} (new, container:{}{})",
                "←".blue(), vr.name, session_branch,
                short_head.dimmed(),
                size_str.as_str().dimmed());
        } else {
            let session_head = git2::Repository::open(host_path).ok()
                .and_then(|repo| {
                    repo.find_reference(&format!("refs/heads/{}", session_branch)).ok()
                        .and_then(|r| r.peel_to_commit().ok())
                        .map(|c| c.id().to_string())
                });
            let from = session_head.as_deref().map(|s| &s[..7]).unwrap_or("?");
            eprintln!("  {} {} → {} ({}..{}{})",
                "←".green(), vr.name, session_branch,
                from.dimmed(), short_head,
                size_str.as_str().dimmed());
        }
    }

    // Full diffstat for changed repos (not new ones — no base to diff against)
    let diffs_to_show: Vec<_> = changed.iter()
        .filter(|(_, _, _, diff, _)| diff.is_some())
        .collect();

    if !diffs_to_show.is_empty() {
        eprintln!();
        render::rule(None);
        eprintln!();
        eprintln!("session diff:");

        let mut total_files = 0u32;
        let mut total_ins = 0u32;
        let mut total_del = 0u32;

        for (vr, _, _, diff, _) in &diffs_to_show {
            if let Some(d) = diff {
                if d.files.is_empty() { continue; }
                eprintln!("  {}", vr.name.as_str().blue());
                let max_path = d.files.iter().map(|f| f.path.len()).max().unwrap_or(20);
                for f in &d.files {
                    let bar = render::render_change_bar_pub(f.insertions, f.deletions, 40);
                    eprintln!("     {:width$} | {:>4} {}", f.path, f.insertions + f.deletions, bar, width = max_path);
                }
                eprintln!("     {} file(s) changed, {} insertions(+), {} deletions(-)",
                    d.files_changed, d.insertions, d.deletions);
                eprintln!();
                total_files += d.files_changed;
                total_ins += d.insertions;
                total_del += d.deletions;
            }
        }

        if total_files > 0 {
            eprintln!("{} Total: {} file(s), +{} -{}", "→".dimmed(), total_files, total_ins, total_del);
        }
    }

    if dry_run {
        return Ok(());
    }

    if !confirm(&format!("\n  Extract {} repo(s)?", changed.len()), auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    // Extract in parallel — repos are independent
    let multi = indicatif::MultiProgress::new();
    let style = indicatif::ProgressStyle::default_spinner()
        .template("  {spinner:.blue} {msg}").unwrap()
        .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏");

    let mut handles = Vec::new();
    for (vr, host_path, session_branch, _, _) in &changed {
        let spinner = multi.add(indicatif::ProgressBar::new_spinner());
        spinner.set_style(style.clone());
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));
        spinner.set_message(format!("Extracting {}...", vr.name));

        let engine_clone = sync::SyncEngine::new(lc.docker_client().clone());
        let name_clone = name.clone();
        let repo_name = vr.name.clone();
        let host_path = host_path.clone();
        let session_branch = session_branch.clone();

        handles.push(tokio::spawn(async move {
            let result = engine_clone.extract(&name_clone, &repo_name, &host_path, &session_branch).await;
            (repo_name, result, spinner)
        }));
    }

    let mut extracted = 0u32;
    let mut failed = 0u32;
    for handle in handles {
        match handle.await {
            Ok((repo_name, Ok(result), spinner)) => {
                spinner.finish_and_clear();
                multi.println(format!("  {} {} ({} commit(s))",
                    "✓".green(), repo_name, result.commit_count)).ok();
                extracted += 1;
            }
            Ok((repo_name, Err(e), spinner)) => {
                spinner.finish_and_clear();
                multi.println(format!("  {} {} — {}",
                    "✗".red(), repo_name, e)).ok();
                failed += 1;
            }
            Err(_) => { failed += 1; }
        }
    }

    eprintln!();
    if failed > 0 {
        eprintln!("  {} {} extracted, {} failed", "⚠".yellow(), extracted, failed);
    } else {
        eprintln!("  {} {} extracted to session branches", "✓".green(), extracted);
    }
    Ok(())
}

pub(crate) async fn cmd_pull(name: &SessionName, branch: &str, filter: Option<&str>, include_deps: bool, dry_run: bool, auto_yes: bool, squash: bool) -> anyhow::Result<()> {
    // Phase 1: Quick preview
    let (lc, engine, initial_plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    let has_work = initial_plan.action.repo_actions.iter().any(|a| !matches!(a.decision, SyncDecision::Skip { .. }));
    let has_extractable = initial_plan.action.repo_actions.iter()
        .any(|a| matches!(a.decision, SyncDecision::Pull { .. } | SyncDecision::CloneToHost | SyncDecision::Reconcile { .. }));

    render::sync_plan_directed(&initial_plan.action, "pull");

    if dry_run || !has_work {
        return Ok(());
    }

    use std::io::Write;

    // Phase 2: Extract ALL repos first
    if has_extractable {
        let session_branch = name.to_string();
        let spinner = indicatif::ProgressBar::new_spinner();
        spinner.set_style(indicatif::ProgressStyle::default_spinner()
            .template("  {spinner:.blue} Fetching {msg}...").unwrap()
            .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"));
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));

        for action in &initial_plan.action.repo_actions {
            if matches!(action.decision, SyncDecision::Skip { .. } | SyncDecision::MergeToTarget { .. }) {
                continue;
            }
            if let Some(host_path) = repo_paths.get(&action.repo_name) {
                spinner.set_message(action.repo_name.clone());
                let _ = engine.extract(name, &action.repo_name, host_path, &session_branch).await;
            }
        }
        spinner.finish_and_clear();
    }

    // Phase 3: Re-plan with accurate data
    let (_lc2, _engine2, plan, _repo_paths2) = build_sync_plan(name, branch, filter, include_deps).await?;

    struct PendingMergeInfo {
        repo_name: String,
        host_path: std::path::PathBuf,
        has_conflict: bool,
        conflict_files: Vec<String>,
    }
    let pending_merge_repos: Vec<PendingMergeInfo> = plan.action.repo_actions.iter()
        .filter(|a| matches!(a.decision, SyncDecision::MergeToTarget { .. }))
        .filter_map(|a| {
            a.host_path.clone().map(|p| {
                let has_conflict = a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty());
                let conflict_files = a.trial_conflicts.clone().unwrap_or_default();
                PendingMergeInfo { repo_name: a.repo_name.clone(), host_path: p, has_conflict, conflict_files }
            })
        })
        .collect();
    struct DivergedInfo {
        repo_name: String,
        container_ahead: u32,
        host_ahead: u32,
        has_conflict: bool,
        conflict_files: Vec<String>,
    }
    let diverged_repos: Vec<DivergedInfo> = plan.action.repo_actions.iter()
        .filter(|a| matches!(a.decision, SyncDecision::Reconcile { .. }))
        .map(|a| {
            let (ca, ha) = match &a.decision {
                SyncDecision::Reconcile { container_ahead, host_ahead } => (*container_ahead, *host_ahead),
                _ => (0, 0),
            };
            let has_conflict = a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty());
            let conflict_files = a.trial_conflicts.clone().unwrap_or_default();
            DivergedInfo { repo_name: a.repo_name.clone(), container_ahead: ca, host_ahead: ha, has_conflict, conflict_files }
        })
        .collect();

    if !pending_merge_repos.is_empty() || !diverged_repos.is_empty() {
        eprintln!();
        render::sync_plan_directed(&plan.action, "pull");
    }

    let (clean_merges, conflict_merges): (Vec<_>, Vec<_>) = pending_merge_repos.iter()
        .partition(|m| !m.has_conflict);

    if !clean_merges.is_empty() {
        let session_branch = name.to_string();
        if confirm(&format!("\n  Merge {} repo(s) into {}?", clean_merges.len(), branch), auto_yes) {
            for m in &clean_merges {
                match engine.merge(&m.host_path, &session_branch, branch, squash) {
                    Ok(outcome) => {
                        if matches!(outcome, git::MergeOutcome::Conflict { .. }) {
                            eprintln!("  {} {} — {}", "✗".red(), m.repo_name, outcome);
                        } else {
                            eprintln!("  {} {} — {}", "✓".green(), m.repo_name, outcome);
                        }
                    }
                    Err(e) => {
                        eprintln!("  {} {} — {}", "✗".red(), m.repo_name, e);
                    }
                }
            }
        }
    }

    if !conflict_merges.is_empty() {
        eprintln!();
        for m in &conflict_merges {
            let file_list = m.conflict_files.iter().take(5).map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
            eprintln!("  {} {} — will conflict ({})", "✗".red(), m.repo_name, file_list);
        }

        let conflict_repos: Vec<_> = conflict_merges.iter()
            .map(|m| (m.repo_name.clone(), m.host_path.clone(), m.conflict_files.clone()))
            .collect();
        offer_reconciliation(&lc, name, &conflict_repos, branch).await?;
    }

    if !diverged_repos.is_empty() {
        eprintln!();
        let mut conflict_repos = Vec::new();

        for dinfo in &diverged_repos {
            let merge_status = if dinfo.has_conflict {
                format!("{}", "CONFLICT".red())
            } else {
                format!("{}", "auto-merge possible".green())
            };

            eprintln!("  {} {} — container +{}, host +{} ({})",
                "↔".yellow(), dinfo.repo_name, dinfo.container_ahead, dinfo.host_ahead, merge_status);

            if dinfo.has_conflict {
                eprint!("    [s]kip  [r]econcile with Claude  > ");
            } else {
                eprint!("    [a]uto-merge  [s]kip  [r]econcile with Claude  > ");
            }
            std::io::stderr().flush().ok();
            let mut choice = String::new();
            std::io::stdin().read_line(&mut choice).ok();
            let choice = choice.trim().to_lowercase();

            match choice.chars().next().unwrap_or('s') {
                'a' if !dinfo.has_conflict => {
                    let host_path = match repo_paths.get(&dinfo.repo_name) {
                        Some(p) => p,
                        None => { eprintln!("    {} no host path", "✗".red()); continue; }
                    };
                    let session_branch = name.to_string();
                    match engine.inject(name, &dinfo.repo_name, host_path, branch).await {
                        Ok(()) => {
                            match engine.extract(name, &dinfo.repo_name, host_path, &session_branch).await {
                                Ok(_extract) => {
                                    match engine.merge(host_path, &session_branch, branch, squash) {
                                        Ok(outcome) => eprintln!("    {} auto-merged ({})", "✓".green(), outcome),
                                        Err(e) => eprintln!("    {} merge failed: {}", "✗".red(), e),
                                    }
                                }
                                Err(e) => eprintln!("    {} extract failed: {}", "✗".red(), e),
                            }
                        }
                        Err(e) => eprintln!("    {} inject failed: {}", "✗".red(), e),
                    }
                }
                'r' => {
                    if let Some(host_path) = repo_paths.get(&dinfo.repo_name) {
                        conflict_repos.push((dinfo.repo_name.clone(), host_path.clone(), dinfo.conflict_files.clone()));
                    }
                }
                _ => {
                    eprintln!("    {} skipped", "·".dimmed());
                }
            }
        }

        if !conflict_repos.is_empty() {
            offer_reconciliation(&lc, name, &conflict_repos, branch).await?;
        }
    }

    Ok(())
}

/// Extract conflict info from sync results for agentic reconciliation.
/// Uses typed pattern matching on RepoSyncResult::Conflicted — no string inspection.
pub(crate) fn collect_conflicts(
    result: &SyncResult,
    repo_paths: &std::collections::BTreeMap<String, std::path::PathBuf>,
) -> Vec<(String, std::path::PathBuf, Vec<String>)> {
    result.results.iter().filter_map(|r| {
        if let action::RepoSyncResult::Conflicted { repo_name, files } = r {
            let host_path = repo_paths.get(repo_name)?.clone();
            Some((repo_name.clone(), host_path, files.clone()))
        } else {
            None
        }
    }).collect()
}

/// Offer agentic reconciliation: launch Claude to resolve merge conflicts.
pub(crate) async fn offer_reconciliation(
    lc: &lifecycle::Lifecycle,
    name: &SessionName,
    conflicts: &[(String, std::path::PathBuf, Vec<String>)],
    branch: &str,
) -> anyhow::Result<()> {
    eprintln!();
    eprintln!("  {} Merge conflicts in {} repo(s):", "⚠".yellow(), conflicts.len());
    for (repo_name, _, files) in conflicts {
        if files.is_empty() {
            eprintln!("    {} {}", "✗".red(), repo_name);
        } else {
            eprintln!("    {} {} ({} file(s))", "✗".red(), repo_name, files.len());
            for f in files.iter().take(5) {
                eprintln!("      {}", f.as_str().dimmed());
            }
            if files.len() > 5 {
                eprintln!("      {} more...", files.len() - 5);
            }
        }
    }

    eprint!("\n  Launch Claude to resolve conflicts? [Y/n] ");
    use std::io::Write;
    std::io::stderr().flush().ok();
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer).ok();
    if answer.trim().to_lowercase().starts_with('n') {
        eprintln!("  Conflicts left unresolved. Fix manually and re-run pull.");
        return Ok(());
    }

    let docker = container::verify_docker(lc).await?;
    let image_ref = ImageRef::new("ghcr.io/hypermemetic/claude-container:latest");
    let verified_image = container::verify_image(lc, &docker, &image_ref).await?;
    let volumes = container::verify_volumes(lc, &docker, name).await?;

    let token = std::env::var("CLAUDE_CODE_OAUTH_TOKEN")
        .or_else(|_| {
            let token_file = dirs::home_dir()
                .unwrap_or_default()
                .join(".config/claude-container/token");
            std::fs::read_to_string(&token_file)
        })
        .map_err(|_| anyhow::anyhow!("No auth token"))?;
    let verified_token = container::verify_token(lc, token.trim())?;

    let ready = verified::LaunchReady {
        docker,
        image: verified_image,
        volumes,
        token: verified_token,
        container: verified::LaunchTarget::Create,
    };

    let script_dir = scripts::materialize()?;

    eprintln!();
    std::io::stderr().flush().ok();

    let reconciled = container::launch_reconciliation(
        lc, ready, name, &script_dir, conflicts,
    ).await?;

    if let Some(_desc) = reconciled {
        eprintln!();
        eprintln!("  {} Reconciliation complete. Re-extracting...", "✓".green());

        let engine = sync::SyncEngine::new(lc.docker_client().clone());
        for (repo_name, host_path, _) in conflicts {
            let session_branch = name.to_string();
            match engine.extract(name, repo_name, host_path, &session_branch).await {
                Ok(extract) => {
                    eprintln!("    {} {} — {} commit(s)", "✓".green(), repo_name, extract.commit_count);
                    match engine.merge(host_path, &session_branch, branch, true) {
                        Ok(outcome) => eprintln!("    {} {} — {}", "✓".green(), repo_name, outcome),
                        Err(e) => eprintln!("    {} {} — merge failed: {}", "✗".red(), repo_name, e),
                    }
                }
                Err(e) => eprintln!("    {} {} — extract failed: {}", "✗".red(), repo_name, e),
            }
        }
    } else {
        eprintln!();
        eprintln!("  {} Claude exited without calling fin. Conflicts unresolved.", "⚠".yellow());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_conflicts_empty_results() {
        let result = SyncResult {
            session_name: SessionName::new("test"),
            results: vec![],
        };
        let paths = std::collections::BTreeMap::new();
        let conflicts = collect_conflicts(&result, &paths);
        assert!(conflicts.is_empty());
    }

    #[test]
    fn collect_conflicts_filters_conflicted_only() {
        let mut paths = std::collections::BTreeMap::new();
        paths.insert("repo-a".to_string(), PathBuf::from("/tmp/repo-a"));
        paths.insert("repo-b".to_string(), PathBuf::from("/tmp/repo-b"));

        let result = SyncResult {
            session_name: SessionName::new("test"),
            results: vec![
                action::RepoSyncResult::Conflicted {
                    repo_name: "repo-a".to_string(),
                    files: vec!["file1.rs".to_string()],
                },
                action::RepoSyncResult::Skipped {
                    repo_name: "repo-b".to_string(),
                    reason: "already in sync".to_string(),
                },
            ],
        };

        let conflicts = collect_conflicts(&result, &paths);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, "repo-a");
        assert_eq!(conflicts[0].2, vec!["file1.rs".to_string()]);
    }

    #[test]
    fn collect_conflicts_skips_missing_paths() {
        let paths = std::collections::BTreeMap::new(); // empty paths

        let result = SyncResult {
            session_name: SessionName::new("test"),
            results: vec![
                action::RepoSyncResult::Conflicted {
                    repo_name: "repo-a".to_string(),
                    files: vec!["file1.rs".to_string()],
                },
            ],
        };

        let conflicts = collect_conflicts(&result, &paths);
        assert!(conflicts.is_empty(), "Should skip conflicts with no host path");
    }

    #[tokio::test]
    #[ignore] // requires Docker
    async fn cmd_extract_is_callable() {
        let name = SessionName::new("test-nonexistent-extract");
        let _ = cmd_extract(&name, None, true, true).await;
    }
}
