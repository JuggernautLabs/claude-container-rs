//! Watch mode — poll container and host for changes, display live feed.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use bollard::Docker;
use colored::Colorize;

use crate::types::{SessionName, CommitHash};

/// A single observed change
#[derive(Debug, Clone)]
pub struct ChangeEvent {
    pub timestamp: Instant,
    pub source: ChangeSource,
    pub repo: String,
    pub kind: ChangeKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ChangeSource {
    Container,
    Host,
}

#[derive(Debug, Clone)]
pub enum ChangeKind {
    NewCommits {
        from: String,  // short SHA
        to: String,
        count: u32,
        message: Option<String>,  // last commit message
    },
    DirtyChanged {
        dirty_files: u32,
    },
    BranchChanged {
        from: String,
        to: String,
    },
    NewRepo,
    Removed,
}

/// Tracked state for one repo
#[derive(Debug, Clone)]
struct RepoState {
    head: String,
    dirty_files: u32,
    branch: Option<String>,
}

/// The watcher — polls and emits events
pub struct Watcher {
    docker: Docker,
    session: SessionName,
    repo_paths: HashMap<String, PathBuf>,  // repo_name → host path
    interval: Duration,
    container_state: HashMap<String, RepoState>,
    host_state: HashMap<String, RepoState>,
    history: Vec<ChangeEvent>,
    started: Instant,
}

impl Watcher {
    pub fn new(
        docker: Docker,
        session: SessionName,
        repo_paths: HashMap<String, PathBuf>,
        interval: Duration,
    ) -> Self {
        Self {
            docker,
            session,
            repo_paths,
            interval,
            container_state: HashMap::new(),
            host_state: HashMap::new(),
            history: Vec::new(),
            started: Instant::now(),
        }
    }

    /// Run the watch loop. Calls `on_event` for each change detected.
    pub async fn run<F>(&mut self, mut on_event: F)
    where
        F: FnMut(&[ChangeEvent], &WatchSummary),
    {
        // Initial poll — seed state without emitting events
        self.poll_container().await;
        self.poll_host();

        let summary = self.summary();
        on_event(&[], &summary);

        loop {
            tokio::time::sleep(self.interval).await;

            let mut new_events = Vec::new();

            // Poll container
            let container_changes = self.poll_container().await;
            new_events.extend(container_changes);

            // Poll host
            let host_changes = self.poll_host();
            new_events.extend(host_changes);

            if !new_events.is_empty() {
                self.history.extend(new_events.clone());
            }

            let summary = self.summary();
            on_event(&new_events, &summary);
        }
    }

    /// Poll all container repos via one docker exec
    async fn poll_container(&mut self) -> Vec<ChangeEvent> {
        let container_name = self.session.container_name();

        // Check if container is running
        let inspect = self.docker.inspect_container(
            container_name.as_str(), None
        ).await;

        let running = inspect.ok()
            .and_then(|i| i.state)
            .and_then(|s| s.running)
            .unwrap_or(false);

        if !running {
            return Vec::new();
        }

        // One exec to get all repo HEADs + dirty counts + branches + last commit msg
        let script = r#"
for d in /workspace/*/ /workspace/*/*/; do
    [ -d "$d/.git" ] || continue
    name="${d#/workspace/}"; name="${name%/}"
    head=$(cd "$d" && git rev-parse --short HEAD 2>/dev/null || echo "?")
    dirty=$(cd "$d" && git status --porcelain 2>/dev/null | wc -l | tr -d ' ')
    branch=$(cd "$d" && git symbolic-ref --short HEAD 2>/dev/null || echo "detached")
    msg=$(cd "$d" && git log -1 --format='%s' 2>/dev/null | head -c 60)
    echo "$name|$head|$dirty|$branch|$msg"
done
"#;

        let exec = self.docker.create_exec(
            container_name.as_str(),
            bollard::exec::CreateExecOptions {
                cmd: Some(vec!["sh".to_string(), "-c".to_string(), script.to_string()]),
                attach_stdout: Some(true),
                attach_stderr: Some(false),
                ..Default::default()
            },
        ).await;

        let exec_id = match exec {
            Ok(e) => e.id,
            Err(_) => return Vec::new(),
        };

        let output = self.docker.start_exec(
            &exec_id,
            None::<bollard::exec::StartExecOptions>,
        ).await;

        let mut stdout = String::new();
        if let Ok(bollard::exec::StartExecResults::Attached { mut output, .. }) = output {
            use futures_util::StreamExt;
            while let Some(Ok(chunk)) = output.next().await {
                stdout.push_str(&chunk.to_string());
            }
        }

        // Parse and compare
        let mut events = Vec::new();
        let now = Instant::now();

        for line in stdout.lines() {
            let parts: Vec<&str> = line.splitn(5, '|').collect();
            if parts.len() < 4 { continue; }
            let name = parts[0].to_string();
            let head = parts[1].to_string();
            let dirty: u32 = parts[2].parse().unwrap_or(0);
            let branch = parts[3].to_string();
            let msg = parts.get(4).map(|s| s.to_string());

            let new_state = RepoState {
                head: head.clone(),
                dirty_files: dirty,
                branch: Some(branch.clone()),
            };

            if let Some(old) = self.container_state.get(&name) {
                if old.head != head {
                    // Count commits between old and new (approximate from exec)
                    events.push(ChangeEvent {
                        timestamp: now,
                        source: ChangeSource::Container,
                        repo: name.clone(),
                        kind: ChangeKind::NewCommits {
                            from: old.head[..7.min(old.head.len())].to_string(),
                            to: head[..7.min(head.len())].to_string(),
                            count: 1, // can't count without another exec
                            message: msg,
                        },
                    });
                }
                if old.branch.as_deref() != Some(&branch) {
                    events.push(ChangeEvent {
                        timestamp: now,
                        source: ChangeSource::Container,
                        repo: name.clone(),
                        kind: ChangeKind::BranchChanged {
                            from: old.branch.clone().unwrap_or("?".into()),
                            to: branch,
                        },
                    });
                }
            } else {
                // New repo appeared
                if !self.container_state.is_empty() {
                    events.push(ChangeEvent {
                        timestamp: now,
                        source: ChangeSource::Container,
                        repo: name.clone(),
                        kind: ChangeKind::NewRepo,
                    });
                }
            }

            self.container_state.insert(name, new_state);
        }

        events
    }

    /// Poll all host repos via git2 (fast, local)
    fn poll_host(&mut self) -> Vec<ChangeEvent> {
        let mut events = Vec::new();
        let now = Instant::now();

        for (name, path) in &self.repo_paths {
            let repo = match git2::Repository::open(path) {
                Ok(r) => r,
                Err(_) => continue,
            };

            let head = repo.head().ok()
                .and_then(|h| h.peel_to_commit().ok())
                .map(|c| c.id().to_string())
                .unwrap_or_default();
            let short_head = head[..7.min(head.len())].to_string();

            let branch = repo.head().ok()
                .and_then(|h| h.shorthand().map(|s| s.to_string()))
                .unwrap_or_else(|| "detached".into());

            let dirty: u32 = repo.statuses(None).ok()
                .map(|s| s.iter().filter(|e| !e.status().is_ignored()).count() as u32)
                .unwrap_or(0);

            let msg = repo.head().ok()
                .and_then(|h| h.peel_to_commit().ok())
                .and_then(|c| c.message().map(|m| m.lines().next().unwrap_or("").to_string()));

            let new_state = RepoState {
                head: short_head.clone(),
                dirty_files: dirty,
                branch: Some(branch.clone()),
            };

            if let Some(old) = self.host_state.get(name) {
                if old.head != short_head {
                    events.push(ChangeEvent {
                        timestamp: now,
                        source: ChangeSource::Host,
                        repo: name.clone(),
                        kind: ChangeKind::NewCommits {
                            from: old.head.clone(),
                            to: short_head,
                            count: 1,
                            message: msg,
                        },
                    });
                }
                if old.dirty_files != dirty {
                    events.push(ChangeEvent {
                        timestamp: now,
                        source: ChangeSource::Host,
                        repo: name.clone(),
                        kind: ChangeKind::DirtyChanged { dirty_files: dirty },
                    });
                }
                if old.branch.as_deref() != Some(&branch) {
                    events.push(ChangeEvent {
                        timestamp: now,
                        source: ChangeSource::Host,
                        repo: name.clone(),
                        kind: ChangeKind::BranchChanged {
                            from: old.branch.clone().unwrap_or("?".into()),
                            to: branch,
                        },
                    });
                }
            }

            self.host_state.insert(name.clone(), new_state);
        }

        events
    }

    /// Current summary state
    pub fn summary(&self) -> WatchSummary {
        let mut synced = 0u32;
        let mut container_ahead = Vec::new();
        let mut host_ahead = Vec::new();
        let mut container_only = Vec::new();

        for (name, ctr) in &self.container_state {
            if let Some(host) = self.host_state.get(name) {
                if ctr.head == host.head {
                    synced += 1;
                } else {
                    // Can't determine direction without ancestry check
                    container_ahead.push(name.clone());
                }
            } else {
                container_only.push(name.clone());
            }
        }

        WatchSummary {
            synced,
            container_ahead,
            host_ahead,
            container_only,
            total_events: self.history.len(),
            uptime: self.started.elapsed(),
        }
    }
}

/// Summary of current watch state
pub struct WatchSummary {
    pub synced: u32,
    pub container_ahead: Vec<String>,
    pub host_ahead: Vec<String>,
    pub container_only: Vec<String>,
    pub total_events: usize,
    pub uptime: Duration,
}

/// Format a change event for display
pub fn format_event(event: &ChangeEvent, start: Instant) -> String {
    let elapsed = event.timestamp.duration_since(start);
    let mins = elapsed.as_secs() / 60;
    let secs = elapsed.as_secs() % 60;
    let time = format!("{:02}:{:02}", mins, secs);

    let source = match event.source {
        ChangeSource::Container => "container".blue().to_string(),
        ChangeSource::Host => "host".green().to_string(),
    };

    let detail = match &event.kind {
        ChangeKind::NewCommits { from, to, count, message } => {
            let msg_part = message.as_deref()
                .map(|m| format!("  \"{}\"", m.chars().take(50).collect::<String>()))
                .unwrap_or_default();
            format!("+{} commit  {}→{}{}", count, from.dimmed(), to, msg_part.dimmed())
        }
        ChangeKind::DirtyChanged { dirty_files } => {
            if *dirty_files > 0 {
                format!("dirty {} file(s)", dirty_files)
            } else {
                "clean".to_string()
            }
        }
        ChangeKind::BranchChanged { from, to } => {
            format!("branch {}→{}", from.dimmed(), to)
        }
        ChangeKind::NewRepo => "new repo".to_string(),
        ChangeKind::Removed => "removed".to_string(),
    };

    format!("  {}  {:9}  {:30}  {}", time.dimmed(), source, event.repo, detail)
}

/// Format the summary line
pub fn format_summary(summary: &WatchSummary) -> String {
    let mut parts = Vec::new();
    if summary.synced > 0 {
        parts.push(format!("{} synced", summary.synced));
    }
    if !summary.container_ahead.is_empty() {
        parts.push(format!("{} changed", summary.container_ahead.len()));
    }
    if !summary.container_only.is_empty() {
        parts.push(format!("{} container-only", summary.container_only.len()));
    }
    if parts.is_empty() {
        parts.push("waiting...".into());
    }
    parts.join(" · ")
}
