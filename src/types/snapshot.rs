//! Snapshot types — the result of reading all container + host state

use std::collections::BTreeMap;
use super::RepoInfo;

/// A complete snapshot of session state
#[derive(Debug)]
pub struct SessionSnapshot {
    pub repos: BTreeMap<String, RepoInfo>,
}

impl SessionSnapshot {
    pub fn new() -> Self {
        Self { repos: BTreeMap::new() }
    }

    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }
}
