//! Repository info — metadata about a repo in the session.
//! Sync state is now in git.rs as RepoPair.

use std::path::PathBuf;

/// A repo's config entry from .claude-projects.yml
#[derive(Debug, Clone)]
pub struct RepoConfig {
    /// Name in config (e.g. "hypermemetic/synapse")
    pub name: String,
    /// Host path
    pub host_path: PathBuf,
    /// Whether extraction is enabled (false = discovered repo)
    pub extract_enabled: bool,
    /// Branch to clone from (None = current HEAD branch)
    pub branch: Option<String>,
}
