//! Plan = state diff. Running a program transforms Vec<RepoVM>.
//! The plan is the predicted transformation. The result is the actual one.
//! They use the same rendering.

use std::collections::BTreeMap;
use super::state::*;

/// A snapshot of all repo states at a point in time.
#[derive(Debug, Clone)]
pub struct Snapshot {
    pub repos: BTreeMap<RepoName, RepoVM>,
}

/// The diff between two snapshots — this IS the plan (predicted) or result (actual).
#[derive(Debug)]
pub struct StateDiff {
    pub repos: Vec<RepoDiff>,
}

/// What changed (or will change) for one repo.
#[derive(Debug)]
pub struct RepoDiff {
    pub name: RepoName,
    pub before: RepoVM,
    pub after: RepoVM,
    pub actions: Vec<String>,  // human-readable descriptions of what happened/will happen
}

impl RepoDiff {
    /// Did anything change?
    pub fn changed(&self) -> bool {
        self.before.container != self.after.container
            || self.before.session != self.after.session
            || self.before.target != self.after.target
            || self.before.conflict != self.after.conflict
    }

    /// What direction(s) of work happened?
    pub fn direction(&self) -> &'static str {
        let container_changed = self.before.container != self.after.container;
        let session_changed = self.before.session != self.after.session;
        let target_changed = self.before.target != self.after.target;

        match (container_changed, target_changed) {
            (true, true) => "↔ sync",
            (true, false) => "→ push",
            (false, true) => "← pull",
            (false, false) if session_changed => "← extract",
            (false, false) => "· unchanged",
        }
    }
}

impl Snapshot {
    pub fn from_vm(vm: &super::SyncVM) -> Self {
        Self { repos: vm.repos.clone() }
    }
}

impl StateDiff {
    /// Compute diff between two snapshots.
    pub fn between(before: &Snapshot, after: &Snapshot, actions: &[String]) -> Self {
        let mut repos = Vec::new();

        for (name, before_repo) in &before.repos {
            let after_repo = after.repos.get(name).cloned()
                .unwrap_or_else(|| before_repo.clone());

            let repo_actions: Vec<String> = actions.iter()
                .filter(|a| a.contains(name.as_str()))
                .cloned()
                .collect();

            repos.push(RepoDiff {
                name: name.clone(),
                before: before_repo.clone(),
                after: after_repo,
                actions: repo_actions,
            });
        }

        // Repos that appear only in after (new repos)
        for (name, after_repo) in &after.repos {
            if !before.repos.contains_key(name) {
                repos.push(RepoDiff {
                    name: name.clone(),
                    before: RepoVM::empty(after_repo.host_path.clone()),
                    after: after_repo.clone(),
                    actions: vec![],
                });
            }
        }

        Self { repos }
    }

    /// How many repos changed?
    pub fn changed_count(&self) -> usize {
        self.repos.iter().filter(|r| r.changed()).count()
    }

    /// How many repos unchanged?
    pub fn unchanged_count(&self) -> usize {
        self.repos.iter().filter(|r| !r.changed()).count()
    }

    /// Render as human-readable text.
    pub fn render(&self, label: &str) -> String {
        let mut out = String::new();
        out.push_str(&format!("── {} ──\n", label));

        let changed: Vec<_> = self.repos.iter().filter(|r| r.changed()).collect();
        let unchanged = self.unchanged_count();

        if changed.is_empty() {
            out.push_str("  Everything in sync.\n");
        } else {
            for diff in &changed {
                out.push_str(&format!("  {} {}\n", diff.direction(), diff.name));
                render_ref_change(&mut out, "container", &diff.before.container, &diff.after.container);
                render_ref_change(&mut out, "session", &diff.before.session, &diff.after.session);
                render_ref_change(&mut out, "target", &diff.before.target, &diff.after.target);
                for action in &diff.actions {
                    out.push_str(&format!("    {}\n", action));
                }
            }
        }

        if unchanged > 0 {
            out.push_str(&format!("  {} unchanged\n", unchanged));
        }

        out
    }
}

fn render_ref_change(out: &mut String, label: &str, before: &RefState, after: &RefState) {
    if before == after { return; }
    let b = ref_short(before);
    let a = ref_short(after);
    out.push_str(&format!("    {}: {} → {}\n", label, b, a));
}

fn ref_short(r: &RefState) -> &str {
    match r {
        RefState::At(h) => &h[..7.min(h.len())],
        RefState::Absent => "absent",
        RefState::Stale => "stale",
    }
}
