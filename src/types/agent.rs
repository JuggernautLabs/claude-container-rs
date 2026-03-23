//! Agent task types — what the container is launched to do

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
    /// Execute a specific task
    Exec {
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
