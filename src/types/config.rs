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
    #[serde(default = "default_true")]
    pub extract: bool,
    #[serde(default)]
    pub main: bool,
}

fn default_true() -> bool { true }

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
