//! Agent task types — what the container is launched to do, how we
//! communicate with Claude, and how Claude signals completion.
//!
//! Communication flow:
//!   HOST → CLAUDE:
//!     1. AgentTask → AGENT_TASK env var + CLAUDE.md injection
//!     2. Initial prompt → .merge-into-summary or positional arg to `claude`
//!     3. RunMode determines interactive vs fire-and-forget
//!
//!   CLAUDE → HOST:
//!     1. Git commits in /workspace (tracked by agent-run pre/post hooks)
//!     2. .agent-result file (repo|commits|head per repo)
//!     3. `fin "description"` command (writes .reconcile-complete, kills container)

/// Why we're launching a container
#[derive(Debug, Clone, PartialEq)]
pub enum AgentTask {
    /// Normal interactive work session
    Work,
    /// Resolve merge conflicts (from pull --reconcile)
    ResolveConflicts {
        target_branch: String,
        summary: String,
        host_mounts: Vec<(String, String)>, // (repo_name, host_path)
    },
    /// Resolve rebase conflicts (from push --rebase)
    RebaseConflicts {
        target_branch: String,
        summary: String,
    },
    /// Code review (read-only)
    Review {
        prompt: Option<String>,
    },
    /// Execute a specific task then exit
    Exec {
        prompt: String,
    },
    /// Run a prompt non-interactively (claude -p), capture output, exit
    Run {
        prompt: String,
    },
}

impl AgentTask {
    pub fn env_value(&self) -> &str {
        match self {
            Self::Work => "work",
            Self::ResolveConflicts { .. } => "resolve-conflicts",
            Self::RebaseConflicts { .. } => "rebase-conflicts",
            Self::Review { .. } => "review",
            Self::Exec { .. } => "exec",
            Self::Run { .. } => "run",
        }
    }

    /// The initial prompt text, if any
    pub fn prompt(&self) -> Option<&str> {
        match self {
            Self::Work => None,
            Self::ResolveConflicts { summary, .. } => Some(summary),
            Self::RebaseConflicts { summary, .. } => Some(summary),
            Self::Review { prompt } => prompt.as_deref(),
            Self::Exec { prompt } => Some(prompt),
            Self::Run { prompt } => Some(prompt),
        }
    }

    /// Whether Claude runs interactively (attached to terminal) or headless
    pub fn run_mode(&self) -> RunMode {
        match self {
            Self::Run { .. } => RunMode::Headless,
            _ => RunMode::Interactive,
        }
    }
}

/// How Claude Code runs inside the container
#[derive(Debug, Clone, PartialEq)]
pub enum RunMode {
    /// Attached to terminal, user can interact (claude "prompt")
    Interactive,
    /// No terminal, runs prompt and exits (claude -p "prompt")
    Headless,
}

impl RunMode {
    /// The claude CLI flag for this mode
    pub fn claude_flag(&self) -> Option<&str> {
        match self {
            Self::Interactive => None,
            Self::Headless => Some("-p"),
        }
    }
}

/// What the agent produced (read from .agent-result after exit)
#[derive(Debug)]
pub struct AgentResult {
    pub repos: Vec<AgentRepoResult>,
    pub total_commits: u32,
    pub changed_repos: u32,
}

#[derive(Debug)]
pub struct AgentRepoResult {
    pub name: String,
    pub commits: u32,
    pub head: super::CommitHash,
}

/// How a container session ended
#[derive(Debug)]
pub enum SessionExit {
    /// Claude exited normally
    Normal { result: AgentResult },
    /// fin command was used (reconcile complete)
    Reconciled { description: String, result: AgentResult },
    /// Container was killed/stopped externally
    Killed,
    /// Entrypoint failed
    EntrypointFailed { exit_code: i32 },
}
