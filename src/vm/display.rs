//! Display implementations for ops — human-readable program preview.

use std::fmt;
use super::ops::*;

impl fmt::Display for Op {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Op::RefRead { side, repo, ref_name } =>
                write!(f, "read {:?} {}/{}", side, repo, ref_name),
            Op::RefWrite { side, repo, ref_name, hash } =>
                write!(f, "write {:?} {}/{} ← {}", side, repo, ref_name, short(hash)),
            Op::TreeCompare { repo, a, b } =>
                write!(f, "compare {} {}..{}", repo, short(a), short(b)),
            Op::AncestryCheck { repo, a, b } =>
                write!(f, "ancestry {} {}..{}", repo, short(a), short(b)),
            Op::MergeTrees { repo, ours, theirs } =>
                write!(f, "merge-trees {} {}+{}", repo, short(ours), short(theirs)),
            Op::Checkout { side, repo, ref_name } =>
                write!(f, "checkout {:?} {} {}", side, repo, ref_name),
            Op::Commit { repo, message, .. } =>
                write!(f, "commit {} \"{}\"", repo, message),
            Op::BundleCreate { repo } =>
                write!(f, "bundle-create {}", repo),
            Op::BundleFetch { repo, .. } =>
                write!(f, "bundle-fetch {}", repo),
            Op::RunContainer { image, .. } =>
                write!(f, "run-container {}", image),
            Op::TryMerge { repo, .. } =>
                write!(f, "try-merge {}", repo),
            Op::AgentRun { repo, task, .. } =>
                write!(f, "agent-run {} {:?}", repo, task_short(task)),
            Op::InteractiveSession { prompt, .. } =>
                write!(f, "interactive-session{}", prompt.as_ref().map(|p| format!(" \"{}\"", truncate(p, 30))).unwrap_or_default()),
            Op::Confirm { message } =>
                write!(f, "confirm \"{}\"", truncate(message, 40)),
        }
    }
}

impl fmt::Display for AgentTask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AgentTask::ResolveConflicts { files } =>
                write!(f, "resolve-conflicts ({} file(s))", files.len()),
            AgentTask::Work =>
                write!(f, "work"),
            AgentTask::Run { prompt } =>
                write!(f, "run \"{}\"", truncate(prompt, 30)),
            AgentTask::Review { prompt } =>
                write!(f, "review \"{}\"", truncate(prompt, 30)),
        }
    }
}

impl fmt::Display for AncestryResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AncestryResult::Same => write!(f, "same"),
            AncestryResult::AIsAncestorOfB { distance } => write!(f, "A ancestor of B (+{})", distance),
            AncestryResult::BIsAncestorOfA { distance } => write!(f, "B ancestor of A (+{})", distance),
            AncestryResult::Diverged { a_ahead, b_ahead, .. } => write!(f, "diverged (A+{}, B+{})", a_ahead, b_ahead),
            AncestryResult::Unknown => write!(f, "unknown"),
        }
    }
}

/// Render a program as a numbered list of steps.
pub fn render_program(ops: &[Op], indent: usize) -> String {
    let mut out = String::new();
    let pad = " ".repeat(indent);
    for (i, op) in ops.iter().enumerate() {
        out.push_str(&format!("{}{}. {}\n", pad, i + 1, op));
        // Show sub-programs for compound ops
        match op {
            Op::TryMerge { on_clean, on_conflict, on_error, .. } => {
                if !on_clean.is_empty() {
                    out.push_str(&format!("{}   on clean:\n", pad));
                    out.push_str(&render_program(on_clean, indent + 5));
                }
                if !on_conflict.is_empty() {
                    out.push_str(&format!("{}   on conflict:\n", pad));
                    out.push_str(&render_program(on_conflict, indent + 5));
                }
                if !on_error.is_empty() {
                    out.push_str(&format!("{}   on error:\n", pad));
                    out.push_str(&render_program(on_error, indent + 5));
                }
            }
            Op::AgentRun { on_success, on_failure, .. } => {
                if !on_success.is_empty() {
                    out.push_str(&format!("{}   on success:\n", pad));
                    out.push_str(&render_program(on_success, indent + 5));
                }
                if !on_failure.is_empty() {
                    out.push_str(&format!("{}   on failure:\n", pad));
                    out.push_str(&render_program(on_failure, indent + 5));
                }
            }
            Op::InteractiveSession { on_exit, .. } => {
                if !on_exit.is_empty() {
                    out.push_str(&format!("{}   on exit:\n", pad));
                    out.push_str(&render_program(on_exit, indent + 5));
                }
            }
            _ => {}
        }
    }
    out
}

fn short(hash: &str) -> &str {
    &hash[..hash.len().min(7)]
}

fn task_short(task: &AgentTask) -> &'static str {
    match task {
        AgentTask::ResolveConflicts { .. } => "resolve-conflicts",
        AgentTask::Work => "work",
        AgentTask::Run { .. } => "run",
        AgentTask::Review { .. } => "review",
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", t)
    }
}
