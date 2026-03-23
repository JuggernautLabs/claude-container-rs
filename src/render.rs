//! Rendering — display plans and state as formatted terminal output.
//! All output goes to stdout. Uses colored crate for formatting.

use colored::*;
use crate::types::*;
use crate::types::git::*;
use crate::types::docker::*;
use crate::types::action::*;

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
    discovered: &session::DiscoveredSession,
    config: Option<&SessionConfig>,
) {
    rule(Some(&format!("session: {}", name)));
    println!();

    match discovered {
        session::DiscoveredSession::DoesNotExist(_) => {
            println!("  {} session does not exist", "✗".red());
        }
        session::DiscoveredSession::VolumesOnly { volumes, metadata, .. } => {
            println!("  container: {}", "none".dimmed());
            render_session_common(name, metadata.as_ref(), config);
        }
        session::DiscoveredSession::Stopped { container, metadata, .. } => {
            println!("  container: {}  ({})", "stopped".dimmed(), name.container_name());
            render_session_common(name, metadata.as_ref(), config);
        }
        session::DiscoveredSession::Running { container, metadata, .. } => {
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
        println!();
        println!("  repos: ({})", cfg.projects.len());
        for (pname, pcfg) in &cfg.projects {
            let path_str = pcfg.path.display().to_string();
            if pcfg.extract {
                println!("    {}  {}", pname.blue(), path_str.dimmed());
            } else {
                println!("    {}  {}  {}", pname.blue(), path_str.dimmed(), "(extract: false)".yellow());
            }
        }
    }
}

/// Render a sync plan
pub fn sync_plan(plan: &SessionSyncPlan) {
    rule(Some(&format!("sync: {} ↔ {}", plan.session_name, plan.target_branch)));
    println!();

    let mut skip_count = 0;

    for action in &plan.repo_actions {
        match &action.decision {
            SyncDecision::Skip { reason } => {
                skip_count += 1;
                // Only show if few repos
                if plan.repo_actions.len() <= 5 {
                    let reason_str = match reason {
                        SkipReason::Identical => "identical",
                        SkipReason::SquashIdentical => "content identical (squash history)",
                        SkipReason::ExtractDisabled => "extract: false",
                    };
                    println!("  {} {} — {}", "✓".green(), action.repo_name, reason_str.dimmed());
                }
            }
            SyncDecision::Pull { commits } => {
                println!("  {} {} — {} commit(s) to pull", "←".blue(), action.repo_name, commits);
                render_diff_lines(&action.outbound_diff);
            }
            SyncDecision::Push { commits } => {
                println!("  {} {} — {} commit(s) to push", "→".blue(), action.repo_name, commits);
                render_diff_lines(&action.inbound_diff);
            }
            SyncDecision::Reconcile { container_ahead, host_ahead } => {
                println!("  {} {} — container +{}, host +{}", "↔".yellow(), action.repo_name, container_ahead, host_ahead);
                render_diff_lines(&action.outbound_diff);
                render_diff_lines(&action.inbound_diff);
            }
            SyncDecision::CloneToHost => {
                println!("  {} {} — clone from container", "←".blue(), action.repo_name);
            }
            SyncDecision::PushToContainer => {
                println!("  {} {} — push to container", "→".blue(), action.repo_name);
            }
            SyncDecision::Blocked { reason } => {
                let reason_str = match reason {
                    BlockReason::ContainerDirty(n) => format!("{} dirty file(s) in container", n),
                    BlockReason::HostDirty => "host has uncommitted changes".into(),
                    BlockReason::ContainerMerging => "merge in progress in container".into(),
                    BlockReason::ContainerRebasing => "rebase in progress in container".into(),
                    BlockReason::HostNotARepo(p) => format!("host path not a git repo: {}", p.display()),
                };
                println!("  {} {} — {}", "!".yellow(), action.repo_name, reason_str);
            }
        }
    }

    println!();
    rule(None);

    if skip_count > 0 && plan.repo_actions.len() > 5 {
        println!("{}", format!("{} already synced", skip_count).dimmed());
    }

    let pulls = plan.pulls().len();
    let pushes = plan.pushes().len();
    let reconciles = plan.reconciles().len();
    let blocked = plan.blocked().len();

    let mut parts = vec![];
    if pulls > 0 { parts.push(format!("{} to pull", pulls)); }
    if pushes > 0 { parts.push(format!("{} to push", pushes)); }
    if reconciles > 0 { parts.push(format!("{} to reconcile", reconciles)); }
    if blocked > 0 { parts.push(format!("{} blocked", blocked)); }

    if parts.is_empty() {
        println!("{}", "✓ Everything in sync".green());
    } else if reconciles > 0 || blocked > 0 {
        println!("{} {}", "⚠".yellow(), parts.join(", "));
    } else {
        println!("{} {}", "→".blue(), parts.join(", "));
    }
}

fn render_diff_lines(diff: &Option<DiffSummary>) {
    if let Some(d) = diff {
        println!("    {}", format!("{}", d).dimmed());
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
