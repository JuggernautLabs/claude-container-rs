//! Session config types (.claude-projects.yml)

use std::path::PathBuf;
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionConfig {
    pub version: Option<String>,
    pub projects: BTreeMap<String, ProjectConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    pub path: PathBuf,
    #[serde(default)]
    pub main: bool,
    #[serde(default)]
    pub role: RepoRole,
}

/// Role determines how a repo participates in sync operations.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RepoRole {
    /// Source of truth — extract, merge, track changes (default)
    Project,
    /// Build dependency — mount in container, push updates in, don't extract
    Dependency,
}

impl Default for RepoRole {
    fn default() -> Self { Self::Project }
}

impl std::fmt::Display for RepoRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project => write!(f, "project"),
            Self::Dependency => write!(f, "dependency"),
        }
    }
}

impl SessionConfig {
    /// Return only project repos (filter out dependencies)
    pub fn project_repos(&self) -> BTreeMap<String, &ProjectConfig> {
        self.projects.iter()
            .filter(|(_, cfg)| cfg.role == RepoRole::Project)
            .map(|(k, v)| (k.clone(), v))
            .collect()
    }

    /// Return only dependency repos
    pub fn dependency_repos(&self) -> BTreeMap<String, &ProjectConfig> {
        self.projects.iter()
            .filter(|(_, cfg)| cfg.role == RepoRole::Dependency)
            .map(|(k, v)| (k.clone(), v))
            .collect()
    }
}

impl SessionConfig {
    /// Find the main project (main: true, or cwd match, or first)
    pub fn main_project(&self, cwd: Option<&PathBuf>) -> Option<&str> {
        // 1. Explicit main: true
        for (name, cfg) in &self.projects {
            if cfg.main {
                return Some(name);
            }
        }
        // 2. Match cwd
        if let Some(cwd) = cwd {
            for (name, cfg) in &self.projects {
                if cwd == &cfg.path || cwd.starts_with(&cfg.path) {
                    return Some(name);
                }
            }
        }
        // 3. First project
        self.projects.keys().next().map(|s| s.as_str())
    }
}
