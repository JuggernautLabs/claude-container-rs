//! Snapshot types — the result of reading all container + host state

use std::collections::BTreeMap;
use super::git::RepoPair;

/// A complete snapshot of session state — every repo as a (container, host) pair
#[derive(Debug)]
pub struct SessionSnapshot {
    pub repos: BTreeMap<String, RepoPair>,
}

impl SessionSnapshot {
    pub fn new() -> Self {
        Self { repos: BTreeMap::new() }
    }

    pub fn repo_count(&self) -> usize {
        self.repos.len()
    }
}
