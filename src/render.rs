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
    let width = 60;
    if let Some(label) = label {
        let label_width = label.chars().count();
        if label_width + 4 >= width {
            // Label too long — just print it without padding
            println!("── {} ──", label);
            return;
        }
        let pad = (width - label_width - 2) / 2;
        let left = "─".repeat(pad);
        let right = "─".repeat(width - pad - label_width - 2);
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

/// Render a sync plan with a specific direction label.
pub fn sync_plan_directed(plan: &SessionSyncPlan, direction: &str) {
    let label = match direction {
        "push" => format!("push: host → {} (container)", plan.session_name),
        "status" => format!("{} ↔ {}", plan.session_name, plan.target_branch),
        "sync" => format!("sync: {} ↔ {}", plan.session_name, plan.target_branch),
        _ => format!("pull: {} → {}", plan.session_name, plan.target_branch),
    };
    sync_plan_inner(plan, &label, direction);
}

fn sync_plan_inner(plan: &SessionSyncPlan, label: &str, direction: &str) {
    rule(Some(label));
    let is_push = direction == "push";

    // Classify actions into groups using two-leg state model
    let mut ready: Vec<&RepoSyncAction> = Vec::new();
    let mut pending_merge: Vec<&RepoSyncAction> = Vec::new();
    let mut diverged: Vec<&RepoSyncAction> = Vec::new();
    let mut skipped: Vec<&RepoSyncAction> = Vec::new();
    let mut blocked: Vec<&RepoSyncAction> = Vec::new();
    let mut unchanged = 0u32;

    for action in &plan.repo_actions {
        if is_push {
            match action.state.push_action() {
                PushAction::Skip => unchanged += 1,
                PushAction::Inject { .. } | PushAction::PushToContainer => ready.push(action),
                PushAction::Blocked(_) => blocked.push(action),
            }
        } else {
            match action.state.pull_action() {
                PullAction::Skip => {
                    // Check if there's push work to show as skipped
                    if matches!(action.state.push_action(), PushAction::Inject { .. } | PushAction::PushToContainer) {
                        skipped.push(action);
                    } else {
                        unchanged += 1;
                    }
                }
                PullAction::Extract { .. } | PullAction::CloneToHost => ready.push(action),
                PullAction::MergeToTarget { .. } => pending_merge.push(action),
                PullAction::Reconcile => diverged.push(action),
                PullAction::Blocked(_) => blocked.push(action),
            }
        }
    }

    // Summary line
    let mut summary_parts = Vec::new();
    if !ready.is_empty() { summary_parts.push(format!("{} ready", ready.len())); }
    let merge_conflicts: Vec<_> = pending_merge.iter().filter(|a| a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty())).collect();
    let merge_clean: Vec<_> = pending_merge.iter().filter(|a| !a.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty())).collect();
    if !merge_clean.is_empty() { summary_parts.push(format!("{} pending merge", merge_clean.len())); }
    if !merge_conflicts.is_empty() { summary_parts.push(format!("{} conflict(s)", merge_conflicts.len())); }
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

    // Ready repos — typed descriptions
    for action in &ready {
        let name = display_name(action);
        let desc = if is_push {
            match action.state.push_action() {
                PushAction::Inject { commits } => format!("{} commit(s) on {} → container", commits, plan.target_branch),
                PushAction::PushToContainer => "push to container".into(),
                _ => String::new(),
            }
        } else {
            match action.state.pull_action() {
                PullAction::Extract { commits } => format!("squash-merge {} commit(s) into {}", commits, plan.target_branch),
                PullAction::CloneToHost => "first extract".into(),
                _ => String::new(),
            }
        };
        println!("  {} {} — {}", "✓".green(), name, desc);
        render_hash_line(action, &plan.target_branch);
        let diff_ref = if is_push { &action.inbound_diff } else { &action.outbound_diff };
        render_diffstat(diff_ref);
    }

    // Pending merge — session branch ahead of target, needs squash-merge
    if !pending_merge.is_empty() {
        if !ready.is_empty() { println!(); }
        for action in &pending_merge {
            let name = display_name(action);
            let ahead = match &action.state.merge {
                MergeLeg::SessionAhead { commits } => *commits,
                MergeLeg::Diverged { session_ahead, .. } => *session_ahead,
                _ => 0,
            };
            let has_conflict = action.trial_conflicts.as_ref().map_or(false, |f| !f.is_empty());
            if has_conflict {
                let files = action.trial_conflicts.as_ref().unwrap();
                let file_list = files.iter().take(3).map(|f| f.as_str()).collect::<Vec<_>>().join(", ");
                println!("  {} {} — {} commit(s) ahead, will conflict ({})",
                    "✗".red(), name, ahead, file_list);
            } else {
                println!("  {} {} — {} commit(s) ahead of {}",
                    "→".blue(), name, ahead, plan.target_branch);
            }
            render_hash_line(action, &plan.target_branch);
            render_diffstat(&action.session_to_target_diff);
        }
    }

    // Diverged repos — both sides changed, need decision
    if !diverged.is_empty() {
        println!();
        for action in &diverged {
            let (ca, ha) = match &action.state.extraction {
                LegState::Diverged { container_ahead, session_ahead } => (*container_ahead, *session_ahead),
                _ => (0, 0),
            };
            println!("  {} {} — container +{}, host +{}", "↔".yellow(), display_name(action), ca, ha);
            render_hash_line(action, &plan.target_branch);
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
            let reason = match action.state.push_action() {
                PushAction::Inject { commits } => format!("host ahead by {} (use push)", commits),
                PushAction::PushToContainer => "push to container".into(),
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
            let reason = match &action.state.blocker {
                Some(b) => format!("{}", b),
                None => "blocked".into(),
            };
            println!("    {} {} ({})", "!".yellow(), display_name(action), reason);
            render_hash_line(action, &plan.target_branch);
            if let Some(ref diff) = action.inbound_diff {
                println!("      {}", format!("{}", diff).dimmed());
            }
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

    fn get_diff<'a>(action: &'a RepoSyncAction, is_push: bool) -> Option<&'a DiffSummary> {
        if is_push { action.inbound_diff.as_ref() } else { action.outbound_diff.as_ref() }
    }

    if ready.iter().any(|a| get_diff(a, is_push).map_or(false, |d| !d.files.is_empty())) {
        println!("session → {} diff:", plan.target_branch);
        has_diffstat = true;
    }

    for action in &ready {
        if let Some(diff) = get_diff(action, is_push) {
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

/// Render a dim line showing container/session/target commit hashes.
fn render_hash_line(action: &RepoSyncAction, target_branch: &str) {
    let mut parts = Vec::new();
    if let Some(ref h) = action.container_head {
        parts.push(format!("container:{}", &h.as_str()[..7.min(h.as_str().len())]));
    }
    if let Some(ref h) = action.session_head {
        parts.push(format!("session:{}", &h.as_str()[..7.min(h.as_str().len())]));
    }
    if let Some(ref h) = action.target_head {
        parts.push(format!("{}:{}", target_branch, &h.as_str()[..7.min(h.as_str().len())]));
    }
    if !parts.is_empty() {
        println!("    {}", parts.join("  ").dimmed());
    }
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
                    "  {} {} — extracted {} commit(s), {}",
                    "✓".green(), repo_name, extract.commit_count, merge
                );
            }
            RepoSyncResult::Merged { repo_name, merge } => {
                println!(
                    "  {} {} — {}",
                    "✓".green(), repo_name, merge
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
