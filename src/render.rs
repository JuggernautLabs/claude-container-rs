//! Rendering — display plans and state as formatted terminal output.
//! All output goes to stdout. Uses colored crate for formatting.

use colored::*;
use crate::types::*;
use crate::types::git::*;
use crate::types::docker::*;
use crate::types::action::*;

pub fn display_name_pub(action: &RepoSyncAction) -> String {
    display_name(action)
}

/// Display name for a repo: relative path from cwd, or absolute if too deep.
fn display_name(action: &RepoSyncAction) -> String {
    if let Some(ref host_path) = action.host_path {
        if let Ok(cwd) = std::env::current_dir() {
            if let Some(rel) = pathdiff::diff_paths(host_path, &cwd) {
                let s = rel.to_string_lossy().to_string();
                // "." or "" = cwd, show as ./reponame
                if s == "." || s.is_empty() {
                    let leaf = action.repo_name.rsplit('/').next().unwrap_or(&action.repo_name);
                    return format!("./{}", leaf);
                }
                if s.matches("../").count() <= 3 {
                    return s;
                }
            }
        }
    }
    action.repo_name.clone()
}

/// Render a horizontal rule
pub fn rule(label: Option<&str>) {
    let width = 50;
    if let Some(label) = label {
        let pad = (width - label.len() - 2) / 2;
        let left = "─".repeat(pad);
        let right = "─".repeat(width - pad - label.len() - 2);
        println!("{} {} {}", left.dimmed(), label, right.dimmed());
    } else {
        println!("{}", "─".repeat(width).dimmed());
    }
}

/// Render session info
pub fn session_info(
    name: &SessionName,
    discovered: &crate::types::DiscoveredSession,
    config: Option<&SessionConfig>,
) {
    rule(Some(&format!("session: {}", name)));
    println!();

    match discovered {
        crate::types::DiscoveredSession::DoesNotExist(_) => {
            println!("  {} session does not exist", "✗".red());
        }
        crate::types::DiscoveredSession::VolumesOnly { volumes, metadata, .. } => {
            println!("  container: {}", "none".dimmed());
            render_session_common(name, metadata.as_ref(), config);
        }
        crate::types::DiscoveredSession::Stopped { container, metadata, .. } => {
            println!("  container: {}  ({})", "stopped".dimmed(), name.container_name());
            render_session_common(name, metadata.as_ref(), config);
        }
        crate::types::DiscoveredSession::Running { container, metadata, .. } => {
            println!("  container: {}  ({})", "running".green(), name.container_name());
            render_session_common(name, metadata.as_ref(), config);
        }
    }

    println!();
    rule(None);
}

fn render_session_common(
    name: &SessionName,
    metadata: Option<&SessionMetadata>,
    config: Option<&SessionConfig>,
) {
    if let Some(meta) = metadata {
        if let Some(ref df) = meta.dockerfile {
            println!("  dockerfile: {}", df.display().to_string().dimmed());
        }
        println!("  rootish: {}", if meta.run_as_rootish { "true".green() } else { "false".dimmed() });
        println!("  docker: {}", if meta.enable_docker { "true".green() } else { "false".dimmed() });
    }
    if let Some(cfg) = config {
        use crate::types::config::RepoRole;
        let projects: Vec<_> = cfg.projects.iter().filter(|(_, c)| c.role == RepoRole::Project).collect();
        let deps: Vec<_> = cfg.projects.iter().filter(|(_, c)| c.role == RepoRole::Dependency).collect();

        if !projects.is_empty() {
            println!();
            println!("  projects: ({})", projects.len());
            for (pname, pcfg) in &projects {
                println!("    {}  {}", pname.blue(), pcfg.path.display().to_string().dimmed());
            }
        }
        if !deps.is_empty() {
            println!();
            println!("  dependencies: ({})", deps.len());
            for (pname, pcfg) in &deps {
                println!("    {}  {}", pname.dimmed(), pcfg.path.display().to_string().dimmed());
            }
        }
        if projects.is_empty() && deps.is_empty() {
            println!();
            println!("  repos: (0)");
        }
    }
}

/// Render a sync plan — matches the bash claude-container UX.
pub fn sync_plan(plan: &SessionSyncPlan) {
    rule(Some(&format!("pull: {} → {}", plan.session_name, plan.target_branch)));

    // Classify actions into groups
    let mut ready: Vec<&RepoSyncAction> = Vec::new();       // clean pull/clone
    let mut pending_merge: Vec<&RepoSyncAction> = Vec::new(); // session branch ahead of target
    let mut diverged: Vec<&RepoSyncAction> = Vec::new();    // both sides changed
    let mut skipped: Vec<&RepoSyncAction> = Vec::new();     // push direction
    let mut blocked: Vec<&RepoSyncAction> = Vec::new();
    let mut unchanged = 0u32;

    for action in &plan.repo_actions {
        // Check if session branch has unmerged work even if container hasn't changed
        if action.session_ahead_of_target > 0 {
            match &action.decision {
                SyncDecision::Skip { .. } => {
                    pending_merge.push(action);
                    continue;
                }
                _ => {} // if there's also new container work, fall through to normal classification
            }
        }

        match &action.decision {
            SyncDecision::Skip { .. } => unchanged += 1,
            SyncDecision::Pull { .. } | SyncDecision::CloneToHost => {
                ready.push(action);
            }
            SyncDecision::Push { .. } | SyncDecision::PushToContainer => {
                skipped.push(action);
            }
            SyncDecision::Reconcile { .. } => {
                diverged.push(action);
            }
            SyncDecision::Blocked { .. } => blocked.push(action),
        }
    }

    // Summary line
    let mut summary_parts = Vec::new();
    if !ready.is_empty() { summary_parts.push(format!("{} ready", ready.len())); }
    if !pending_merge.is_empty() { summary_parts.push(format!("{} pending merge", pending_merge.len())); }
    if !diverged.is_empty() { summary_parts.push(format!("{} diverged", diverged.len())); }
    if !skipped.is_empty() { summary_parts.push(format!("{} skipped", skipped.len())); }
    if !blocked.is_empty() { summary_parts.push(format!("{} blocked", blocked.len())); }

    if !summary_parts.is_empty() {
        let icon = if !diverged.is_empty() || !blocked.is_empty() { "⚠".yellow() } else { "✓".green() };
        println!("{} {}", icon, summary_parts.join(", "));
    }
    if unchanged > 0 {
        println!("{}", format!("{} unchanged", unchanged).dimmed());
    }
    println!();

    // Ready repos — clean pulls
    for action in &ready {
        let name = display_name(action);
        let desc = match &action.decision {
            SyncDecision::Pull { commits } => format!("squash-merge {} commit(s) into {}", commits, plan.target_branch),
            SyncDecision::CloneToHost => "first extract".into(),
            _ => String::new(),
        };
        println!("  {} {} — {}", "✓".green(), name, desc);
        render_diffstat(&action.outbound_diff);
    }

    // Pending merge — session branch ahead of target, needs squash-merge
    if !pending_merge.is_empty() {
        if !ready.is_empty() { println!(); }
        for action in &pending_merge {
            let name = display_name(action);
            println!("  {} {} — session branch {} commit(s) ahead of {}",
                "→".blue(), name, action.session_ahead_of_target, plan.target_branch);
            render_diffstat(&action.session_to_target_diff);
        }
    }

    // Diverged repos — both sides changed, need decision
    if !diverged.is_empty() {
        println!();
        for action in &diverged {
            let (ca, ha) = match &action.decision {
                SyncDecision::Reconcile { container_ahead, host_ahead } => (*container_ahead, *host_ahead),
                _ => (0, 0),
            };
            println!("  {} {} — container +{}, host +{}", "↔".yellow(), display_name(action), ca, ha);
            render_diffstat(&action.outbound_diff);

            // Show trial merge result
            match &action.trial_conflicts {
                Some(files) if files.is_empty() => {
                    println!("      trial merge: {} — auto-merge possible", "clean".green());
                }
                Some(files) => {
                    let file_list = files.iter().take(5).map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
                    println!("      trial merge: {} ({})", "CONFLICT".red(), file_list);
                }
                None => {
                    println!("      trial merge: {} — container commit not on host yet", "unknown".dimmed());
                }
            }
        }
    }

    // Skipped repos
    if !skipped.is_empty() {
        println!();
        println!("  {}:", "skipped".dimmed());
        for action in &skipped {
            let reason = match &action.decision {
                SyncDecision::Push { commits } => format!("host ahead by {} (use push)", commits),
                SyncDecision::PushToContainer => "push to container".into(),
                _ => "skipped".into(),
            };
            println!("    {} — {}", display_name(action), reason.dimmed());
        }
    }

    // Blocked repos
    if !blocked.is_empty() {
        println!();
        println!("  {}:", "blocked".dimmed());
        for action in &blocked {
            let reason = match &action.decision {
                SyncDecision::Blocked { reason } => match reason {
                    BlockReason::ContainerDirty(n) => format!("{} dirty file(s) in container", n),
                    BlockReason::HostDirty => "host has uncommitted changes".into(),
                    BlockReason::ContainerMerging => "merge in progress in container".into(),
                    BlockReason::ContainerRebasing => "rebase in progress in container".into(),
                    BlockReason::HostNotARepo(p) => format!("host path not a git repo: {}", p.display()),
                },
                _ => "blocked".into(),
            };
            println!("    {} {} ({})", "!".yellow(), display_name(action), reason);
        }
    }

    println!();
    rule(None);
    println!();

    // Full diffstat per ready repo
    let mut total_files = 0u32;
    let mut total_ins = 0u32;
    let mut total_del = 0u32;
    let mut has_diffstat = false;

    if let Some(diff) = ready.iter().find_map(|a| a.outbound_diff.as_ref()) {
        // Only show full diffstat section if there are actual diffs
        if ready.iter().any(|a| a.outbound_diff.as_ref().map_or(false, |d| !d.files.is_empty())) {
            println!("session → {} diff:", plan.target_branch);
            has_diffstat = true;
        }
    }

    for action in &ready {
        if let Some(diff) = &action.outbound_diff {
            if diff.files.is_empty() { continue; }
            println!("  {}", display_name(action).blue());

            // Find max path length for alignment
            let max_path = diff.files.iter().map(|f| f.path.len()).max().unwrap_or(20);

            for f in &diff.files {
                let bar = render_change_bar(f.insertions, f.deletions, 40);
                println!("     {:width$} | {:>4} {}", f.path, f.insertions + f.deletions, bar, width = max_path);
            }
            println!("     {} file(s) changed, {} insertions(+), {} deletions(-)",
                diff.files_changed, diff.insertions, diff.deletions);
            println!();

            total_files += diff.files_changed;
            total_ins += diff.insertions;
            total_del += diff.deletions;
        }
    }

    if has_diffstat && (total_files > 0) {
        println!("{} Total: {} file(s), +{} -{}", "→".dimmed(), total_files, total_ins, total_del);
    }
}

/// Render a +/- bar like git diff --stat (public for use in main.rs)
pub fn render_change_bar_pub(insertions: u32, deletions: u32, max_width: u32) -> String {
    render_change_bar(insertions, deletions, max_width)
}

fn render_change_bar(insertions: u32, deletions: u32, max_width: u32) -> String {
    let total = insertions + deletions;
    if total == 0 { return String::new(); }

    let scale = if total > max_width { max_width as f64 / total as f64 } else { 1.0 };
    let plus_count = (insertions as f64 * scale).ceil() as usize;
    let minus_count = (deletions as f64 * scale).ceil() as usize;

    format!("{}{}",
        "+".repeat(plus_count).green(),
        "-".repeat(minus_count).red(),
    )
}

fn render_diffstat(diff: &Option<DiffSummary>) {
    if let Some(d) = diff {
        if !d.files.is_empty() {
            println!("      {} files changed, {} insertions(+), {} deletions(-)",
                d.files_changed, d.insertions, d.deletions);
        }
    }
}

fn render_diff_lines(diff: &Option<DiffSummary>) {
    if let Some(d) = diff {
        println!("    {}", format!("{}", d).dimmed());
        for f in &d.files {
            let marker = match &f.status {
                crate::types::action::FileStatus::Added => "A".green(),
                crate::types::action::FileStatus::Deleted => "D".red(),
                crate::types::action::FileStatus::Modified => "M".yellow(),
                crate::types::action::FileStatus::Renamed(old) => {
                    println!("    {} {} → {}", "R".blue(), old.dimmed(), f.path);
                    continue;
                }
            };
            if f.insertions > 0 || f.deletions > 0 {
                println!("    {} {} +{} -{}", marker, f.path, f.insertions, f.deletions);
            } else {
                println!("    {} {}", marker, f.path);
            }
        }
    }
}

fn render_trial_conflicts(trial: &Option<Vec<String>>) {
    match trial {
        Some(files) if !files.is_empty() => {
            println!("    {} merge will conflict ({} file(s)):", "⚠".yellow(), files.len());
            for f in files.iter().take(5) {
                println!("      {}", f.red());
            }
            if files.len() > 5 {
                println!("      {} more...", files.len() - 5);
            }
        }
        Some(_) => {
            // Empty = clean merge, no need to print anything
        }
        None => {
            // Trial merge not performed or not possible
        }
    }
}

/// Render container launch plan
pub fn container_plan(plan: &ContainerPlan) {
    match &plan.action {
        ContainerAction::Create { image, .. } => {
            println!("{} Creating container from {}", "→".blue(), image);
        }
        ContainerAction::Resume { container } => {
            println!("{} Resuming {}", "→".blue(), container);
        }
        ContainerAction::Rebuild { container, reasons, .. } => {
            println!("{} Container needs rebuild:", "⚠".yellow());
            for reason in reasons {
                println!("  {} {}", "·".dimmed(), reason);
            }
        }
        ContainerAction::Attach { container } => {
            println!("{} Attaching to running {}", "→".blue(), container);
        }
    }
}

/// Render image validation
pub fn image_validation(v: &ImageValidation) {
    if v.is_valid() {
        println!("{} Image valid", "✓".green());
    } else {
        println!("{} Image invalid:", "✗".red());
        for tool in v.missing_critical() {
            println!("  {} {}", "✗".red(), tool);
        }
    }
    for tool in v.missing_optional() {
        println!("  {} {} (optional)", "⚠".yellow(), tool);
    }
}

/// Render sync execution results
pub fn sync_result(result: &crate::types::SyncResult) {
    use crate::types::action::RepoSyncResult;

    rule(Some("results"));
    println!();

    for r in &result.results {
        match r {
            RepoSyncResult::Pulled { repo_name, extract, merge } => {
                println!(
                    "  {} {} — extracted {} commit(s), {:?}",
                    "✓".green(), repo_name, extract.commit_count, merge
                );
            }
            RepoSyncResult::Pushed { repo_name } => {
                println!("  {} {} — pushed", "✓".green(), repo_name);
            }
            RepoSyncResult::ClonedToHost { repo_name, extract } => {
                println!(
                    "  {} {} — cloned to host ({} commit(s))",
                    "✓".green(), repo_name, extract.commit_count
                );
            }
            RepoSyncResult::Skipped { repo_name, reason } => {
                println!("  {} {} — {}", "·".dimmed(), repo_name, reason.dimmed());
            }
            RepoSyncResult::Conflicted { repo_name, files } => {
                println!("  {} {} — merge conflict ({} file(s))", "✗".red(), repo_name, files.len());
                for f in files.iter().take(5) {
                    println!("      {}", f.dimmed());
                }
                if files.len() > 5 {
                    println!("      {} more...", files.len() - 5);
                }
            }
            RepoSyncResult::Failed { repo_name, error } => {
                println!("  {} {} — {}", "✗".red(), repo_name, error);
            }
        }
    }

    println!();
    rule(None);

    let s = result.succeeded();
    let f = result.failed();
    let c = result.conflicted();
    let k = result.skipped();

    if result.is_partial() {
        println!("{} Partial sync: {} succeeded, {} failed, {} conflict(s), {} skipped",
            "⚠".yellow(), s, f, c, k);
    } else if f > 0 || c > 0 {
        println!("{} {} succeeded, {} failed, {} conflict(s), {} skipped",
            "⚠".yellow(), s, f, c, k);
    } else if s > 0 {
        println!("{} {} succeeded, {} skipped", "✓".green(), s, k);
    } else {
        println!("{}", "Nothing to do.".dimmed());
    }
}
