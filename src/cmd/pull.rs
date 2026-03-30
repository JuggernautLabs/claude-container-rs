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
use super::sync_cmd::{build_sync_plan, build_vm_from_plan};

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

        let host_session_head = git2::Repository::open(host_path).ok()
            .and_then(|repo| {
                repo.find_reference(&format!("refs/heads/{}", session_branch)).ok()
                    .and_then(|r| r.peel_to_commit().ok())
                    .map(|c| CommitHash::new(c.id().to_string()))
            });

        let container_head = &vr.head;
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

    render::rule(Some(&format!("extract: {}", name)));
    if changed.is_empty() {
        eprintln!("{}", "Nothing new to extract.".dimmed());
        return Ok(());
    }

    eprintln!("{} to extract, {} unchanged", changed.len(), unchanged);
    eprintln!();

    for (vr, host_path, session_branch, _, status) in &changed {
        let short_head = &vr.head.as_str()[..7.min(vr.head.as_str().len())];
        let size_str = if vr.git_size_mb > 0 { format!(" {}MB", vr.git_size_mb) } else { String::new() };
        if *status == "new" {
            eprintln!("  {} {} → {} (new, container:{}{})",
                "←".blue(), vr.name, session_branch, short_head.dimmed(), size_str.as_str().dimmed());
        } else {
            let session_head = git2::Repository::open(host_path).ok()
                .and_then(|repo| {
                    repo.find_reference(&format!("refs/heads/{}", session_branch)).ok()
                        .and_then(|r| r.peel_to_commit().ok())
                        .map(|c| c.id().to_string())
                });
            let from = session_head.as_deref().map(|s| &s[..7]).unwrap_or("?");
            eprintln!("  {} {} → {} ({}..{}{})",
                "←".green(), vr.name, session_branch, from.dimmed(), short_head, size_str.as_str().dimmed());
        }
    }

    if dry_run { return Ok(()); }

    if !confirm(&format!("\n  Extract {} repo(s)?", changed.len()), auto_yes) {
        eprintln!("  Aborted.");
        return Ok(());
    }

    // Extract via VM
    let backend = git_sandbox::vm::RealBackend::from_docker(lc.docker_client().clone(), name.as_str());
    let mut vm = git_sandbox::vm::SyncVM::new(name.as_str(), "");

    let mut extracted = 0u32;
    let mut failed = 0u32;
    for (vr, host_path, session_branch, _, _) in &changed {
        vm.set_repo(&vr.name, git_sandbox::vm::RepoVM::from_refs(
            git_sandbox::vm::RefState::At(vr.head.as_str().to_string()),
            git_sandbox::vm::RefState::Absent,
            git_sandbox::vm::RefState::Absent,
            Some(host_path.clone()),
        ));

        let result = vm.run(&backend, vec![
            git_sandbox::vm::Op::Extract { repo: vr.name.clone(), session_branch: session_branch.clone() },
        ]).await;

        if result.succeeded() > 0 {
            eprintln!("  {} {}", "✓".green(), vr.name);
            extracted += 1;
        } else {
            let err = result.outcomes.first()
                .and_then(|o| match &o.result {
                    git_sandbox::vm::StepResult::BackendError(e) => Some(e.as_str()),
                    _ => None,
                })
                .unwrap_or("unknown error");
            eprintln!("  {} {} — {}", "✗".red(), vr.name, err);
            failed += 1;
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
    let (lc, engine, initial_plan, repo_paths) = build_sync_plan(name, branch, filter, include_deps).await?;

    let has_work = initial_plan.action.repo_actions.iter().any(|a| a.state.has_work());
    let has_extractable = initial_plan.action.repo_actions.iter()
        .any(|a| matches!(a.state.pull_action(), PullAction::Extract { .. } | PullAction::CloneToHost | PullAction::Reconcile));

    render::sync_plan_directed(&initial_plan.action, "pull");

    if dry_run || !has_work { return Ok(()); }

    use std::io::Write;

    // Phase 1: Extract via VM
    let backend = git_sandbox::vm::RealBackend::from_docker(lc.docker_client().clone(), name.as_str());
    let plan = if has_extractable {
        let mut vm = build_vm_from_plan(name, branch, &initial_plan.action, &repo_paths);
        let extract_ops: Vec<_> = initial_plan.action.repo_actions.iter()
            .filter(|a| matches!(a.state.pull_action(), PullAction::Extract { .. } | PullAction::CloneToHost | PullAction::Reconcile))
            .map(|a| git_sandbox::vm::Op::Extract {
                repo: a.repo_name.clone(),
                session_branch: name.to_string(),
            })
            .collect();

        if !extract_ops.is_empty() {
            let spinner = indicatif::ProgressBar::new_spinner();
            spinner.set_style(indicatif::ProgressStyle::default_spinner()
                .template("  {spinner:.blue} Extracting...").unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"));
            spinner.enable_steady_tick(std::time::Duration::from_millis(80));

            let result = vm.run(&backend, extract_ops).await;
            spinner.finish_and_clear();

            if result.failed() > 0 {
                for o in &result.outcomes {
                    if !o.result.is_ok() {
                        eprintln!("  {} extract: {}", "✗".red(), o.op_description);
                    }
                }
            }
        }

        // Re-plan with accurate trial merges
        let (_lc2, _engine2, plan, _rp2) = build_sync_plan(name, branch, filter, include_deps).await?;
        render::sync_plan_directed(&plan.action, "pull");
        plan
    } else {
        initial_plan
    };

    // Phase 2: Merge — use engine.merge() directly (VM TryMerge needs data flow)
    let pending_merges: Vec<_> = plan.action.repo_actions.iter()
        .filter(|a| matches!(a.state.pull_action(), PullAction::MergeToTarget { .. }))
        .filter_map(|a| a.host_path.clone().map(|p| {
            let has_conflict = a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty());
            let conflict_files = a.trial_conflicts.clone().unwrap_or_default();
            (a.repo_name.clone(), p, has_conflict, conflict_files)
        }))
        .collect();

    let (clean, conflicts): (Vec<_>, Vec<_>) = pending_merges.iter()
        .partition(|(_, _, has_conflict, _)| !has_conflict);

    if !clean.is_empty() {
        let session_branch = name.to_string();
        if confirm(&format!("\n  Merge {} repo(s) into {}?", clean.len(), branch), auto_yes) {
            for (repo_name, host_path, _, _) in &clean {
                match engine.merge(host_path, &session_branch, branch, squash) {
                    Ok(outcome) => {
                        if matches!(outcome, git::MergeOutcome::Conflict { .. }) {
                            eprintln!("  {} {} — {}", "✗".red(), repo_name, outcome);
                        } else {
                            eprintln!("  {} {} — {}", "✓".green(), repo_name, outcome);
                        }
                    }
                    Err(e) => eprintln!("  {} {} — {}", "✗".red(), repo_name, e),
                }
            }
        }
    }

    if !conflicts.is_empty() {
        eprintln!();
        for (repo_name, _, _, conflict_files) in &conflicts {
            let file_list = conflict_files.iter().take(5).map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
            eprintln!("  {} {} — will conflict ({})", "✗".red(), repo_name, file_list);
        }
        let conflict_repos: Vec<_> = conflicts.iter()
            .map(|(name, path, _, files)| (name.clone(), path.clone(), files.clone()))
            .collect();
        offer_reconciliation(&lc, name, &conflict_repos, branch).await?;
    }

    // Phase 3: Diverged repos — interactive
    let diverged: Vec<_> = plan.action.repo_actions.iter()
        .filter(|a| matches!(a.state.pull_action(), PullAction::Reconcile))
        .map(|a| {
            let (ca, ha) = match &a.state.extraction {
                LegState::Diverged { container_ahead, session_ahead } => (*container_ahead, *session_ahead),
                _ => (0, 0),
            };
            let has_conflict = a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty());
            let conflict_files = a.trial_conflicts.clone().unwrap_or_default();
            (a.repo_name.clone(), ca, ha, has_conflict, conflict_files)
        })
        .collect();

    if !diverged.is_empty() {
        eprintln!();
        let mut conflict_repos = Vec::new();

        for (repo_name, ca, ha, has_conflict, conflict_files) in &diverged {
            let merge_status = if *has_conflict {
                format!("{}", "CONFLICT".red())
            } else {
                format!("{}", "auto-merge possible".green())
            };
            eprintln!("  {} {} — container +{}, host +{} ({})", "↔".yellow(), repo_name, ca, ha, merge_status);

            if *has_conflict {
                eprint!("    [s]kip  [r]econcile with Claude  > ");
            } else {
                eprint!("    [a]uto-merge  [s]kip  [r]econcile with Claude  > ");
            }
            std::io::stderr().flush().ok();
            let mut choice = String::new();
            std::io::stdin().read_line(&mut choice).ok();
            let choice = choice.trim().to_lowercase();

            match choice.chars().next().unwrap_or('s') {
                'a' if !has_conflict => {
                    let host_path = match repo_paths.get(repo_name) {
                        Some(p) => p,
                        None => { eprintln!("    {} no host path", "✗".red()); continue; }
                    };
                    // Auto-reconcile via VM: inject + extract + merge
                    let mut vm = build_vm_from_plan(name, branch, &plan.action, &repo_paths);
                    let ops = vec![
                        git_sandbox::vm::Op::Inject { repo: repo_name.clone(), branch: branch.to_string() },
                        git_sandbox::vm::Op::Extract { repo: repo_name.clone(), session_branch: name.to_string() },
                    ];
                    let result = vm.run(&backend, ops).await;
                    if result.failed() > 0 {
                        eprintln!("    {} reconcile failed", "✗".red());
                    } else {
                        let session_branch = name.to_string();
                        match engine.merge(host_path, &session_branch, branch, squash) {
                            Ok(outcome) => eprintln!("    {} auto-merged ({})", "✓".green(), outcome),
                            Err(e) => eprintln!("    {} merge failed: {}", "✗".red(), e),
                        }
                    }
                }
                'r' => {
                    if let Some(host_path) = repo_paths.get(repo_name) {
                        conflict_repos.push((repo_name.clone(), host_path.clone(), conflict_files.clone()));
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
            let token_file = dirs::home_dir().unwrap_or_default()
                .join(".config/claude-container/token");
            std::fs::read_to_string(&token_file)
        })
        .map_err(|_| anyhow::anyhow!("No auth token"))?;
    let verified_token = container::verify_token(lc, token.trim())?;

    let ready = verified::LaunchReady {
        docker, image: verified_image, volumes, token: verified_token,
        container: verified::LaunchTarget::Create,
    };

    let script_dir = scripts::materialize()?;
    eprintln!();
    std::io::stderr().flush().ok();

    let reconciled = container::launch_reconciliation(lc, ready, name, &script_dir, conflicts).await?;

    if let Some(_desc) = reconciled {
        eprintln!();
        eprintln!("  {} Reconciliation complete. Re-extracting...", "✓".green());

        let backend = git_sandbox::vm::RealBackend::from_docker(lc.docker_client().clone(), name.as_str());
        let mut vm = git_sandbox::vm::SyncVM::new(name.as_str(), branch);

        for (repo_name, host_path, _) in conflicts {
            vm.set_repo(repo_name, git_sandbox::vm::RepoVM::from_refs(
                git_sandbox::vm::RefState::Stale, // container state changed by agent
                git_sandbox::vm::RefState::Absent,
                git_sandbox::vm::RefState::Absent,
                Some(host_path.clone()),
            ));

            let result = vm.run(&backend, vec![
                git_sandbox::vm::Op::Extract { repo: repo_name.clone(), session_branch: name.to_string() },
            ]).await;

            if result.succeeded() > 0 {
                eprintln!("    {} {} — extracted", "✓".green(), repo_name);
                let engine = sync::SyncEngine::new(lc.docker_client().clone());
                let session_branch = name.to_string();
                match engine.merge(host_path, &session_branch, branch, true) {
                    Ok(outcome) => eprintln!("    {} {} — {}", "✓".green(), repo_name, outcome),
                    Err(e) => eprintln!("    {} {} — merge failed: {}", "✗".red(), repo_name, e),
                }
            } else {
                eprintln!("    {} {} — extract failed", "✗".red(), repo_name);
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
        let result = SyncResult { session_name: SessionName::new("test"), results: vec![] };
        let paths = std::collections::BTreeMap::new();
        assert!(collect_conflicts(&result, &paths).is_empty());
    }

    #[test]
    fn collect_conflicts_filters_conflicted_only() {
        let mut paths = std::collections::BTreeMap::new();
        paths.insert("repo-a".to_string(), PathBuf::from("/tmp/repo-a"));
        paths.insert("repo-b".to_string(), PathBuf::from("/tmp/repo-b"));
        let result = SyncResult {
            session_name: SessionName::new("test"),
            results: vec![
                action::RepoSyncResult::Conflicted { repo_name: "repo-a".to_string(), files: vec!["file1.rs".to_string()] },
                action::RepoSyncResult::Skipped { repo_name: "repo-b".to_string(), reason: "already in sync".to_string() },
            ],
        };
        let conflicts = collect_conflicts(&result, &paths);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].0, "repo-a");
    }

    #[test]
    fn collect_conflicts_skips_missing_paths() {
        let paths = std::collections::BTreeMap::new();
        let result = SyncResult {
            session_name: SessionName::new("test"),
            results: vec![
                action::RepoSyncResult::Conflicted { repo_name: "repo-a".to_string(), files: vec!["file1.rs".to_string()] },
            ],
        };
        assert!(collect_conflicts(&result, &paths).is_empty());
    }

    #[tokio::test]
    #[ignore]
    async fn cmd_extract_is_callable() {
        let name = SessionName::new("test-nonexistent-extract");
        let _ = cmd_extract(&name, None, true, true).await;
    }
}
