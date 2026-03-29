//! Primitive-level backend trait — one method per op.
//!
//! The VM dispatches each Op to the corresponding backend method.
//! RealBackend wraps SyncEngine. MockBackend returns canned responses.

use std::path::Path;
use super::ops::{Op, OpResult, Mount};

/// Errors from backend execution.
#[derive(Debug)]
pub enum VmBackendError {
    /// Operation failed
    Failed(String),
}

impl std::fmt::Display for VmBackendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(msg) => write!(f, "{}", msg),
        }
    }
}

impl std::error::Error for VmBackendError {}

/// Primitive-level backend — one method per atomic operation.
///
/// Sync methods for git2 ops, async would be needed for Docker ops.
/// For now, all methods are sync — Docker ops will be adapted when
/// we wire up the real backend.
pub trait VmBackend: Send + Sync {
    // ── Ref ops ──
    fn ref_read(&self, repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError>;
    fn ref_write(&self, repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError>;

    // ── Tree ops ──
    fn tree_compare(&self, repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError>;
    fn ancestry_check(&self, repo_path: &Path, a: &str, b: &str) -> Result<super::AncestryResult, VmBackendError>;
    fn merge_trees(&self, repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError>;
    fn checkout(&self, repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError>;
    fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], message: &str) -> Result<String, VmBackendError>;

    // ── Transport ops ──
    fn bundle_create(&self, session: &str, repo: &str) -> Result<String, VmBackendError>;
    fn bundle_fetch(&self, repo_path: &Path, bundle_path: &str) -> Result<String, VmBackendError>;

    // ── Container ops ──
    fn run_container(&self, image: &str, script: &str, mounts: &[Mount]) -> Result<(i64, String), VmBackendError>;
    fn attach_container(&self, image: &str, env: &[(String, String)], mounts: &[Mount]) -> Result<i64, VmBackendError>;

    // ── Control ──
    fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError>;
}

/// Mock backend for unit tests — records calls, returns canned responses.
#[derive(Debug)]
pub struct MockBackend {
    /// Canned responses keyed by a description of the call.
    pub responses: std::sync::Mutex<Vec<MockResponse>>,
    /// Recorded calls.
    pub calls: std::sync::Mutex<Vec<String>>,
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            responses: std::sync::Mutex::new(Vec::new()),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

/// A canned response for the mock backend.
#[derive(Debug, Clone)]
pub struct MockResponse {
    /// Match pattern (substring of the call description)
    pub pattern: String,
    /// The result to return
    pub result: MockResult,
}

/// Possible mock results.
#[derive(Debug, Clone)]
pub enum MockResult {
    Hash(String),
    Bool(bool),
    Comparison(bool, u32),
    Ancestry(super::AncestryResult),
    MergeClean(String),
    MergeConflict(Vec<String>),
    ContainerOutput(i64, String),
    ContainerExited(i64),
    Unit,
    Error(String),
}

impl MockBackend {
    pub fn new() -> Self { Self::default() }

    /// Add a canned response.
    pub fn on(&self, pattern: &str, result: MockResult) {
        self.responses.lock().unwrap().push(MockResponse {
            pattern: pattern.to_string(),
            result,
        });
    }

    fn pop_response(&self, call: &str) -> Option<MockResult> {
        let mut responses = self.responses.lock().unwrap();
        if let Some(idx) = responses.iter().position(|r| call.contains(&r.pattern)) {
            Some(responses.remove(idx).result)
        } else {
            None
        }
    }

    fn record(&self, call: &str) {
        self.calls.lock().unwrap().push(call.to_string());
    }

    /// Get recorded calls (for assertions).
    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl VmBackend for MockBackend {
    fn ref_read(&self, _repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError> {
        let call = format!("ref_read:{}", ref_name);
        // Cast away mutability for recording — tests are single-threaded
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Hash(h)) => Ok(Some(h)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(None),
        }
    }

    fn ref_write(&self, _repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
        let call = format!("ref_write:{}={}", ref_name, hash);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    fn tree_compare(&self, _repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
        let call = format!("tree_compare:{}..{}", a, b);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Comparison(identical, files)) => Ok((identical, files)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, 0)),
        }
    }

    fn ancestry_check(&self, _repo_path: &Path, a: &str, b: &str) -> Result<super::AncestryResult, VmBackendError> {
        let call = format!("ancestry_check:{}..{}", a, b);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Ancestry(r)) => Ok(r),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(super::AncestryResult::Unknown),
        }
    }

    fn merge_trees(&self, _repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError> {
        let call = format!("merge_trees:{}+{}", ours, theirs);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::MergeClean(tree)) => Ok((true, Some(tree), vec![])),
            Some(MockResult::MergeConflict(files)) => Ok((false, None, files)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, Some("mock_tree".into()), vec![])),
        }
    }

    fn checkout(&self, _repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError> {
        let call = format!("checkout:{}", ref_name);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    fn commit(&self, _repo_path: &Path, _tree: &str, _parents: &[String], msg: &str) -> Result<String, VmBackendError> {
        let call = format!("commit:{}", msg);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Hash(h)) => Ok(h),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok("mock_commit_hash".into()),
        }
    }

    fn bundle_create(&self, _session: &str, repo: &str) -> Result<String, VmBackendError> {
        let call = format!("bundle_create:{}", repo);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Hash(h)) => Ok(h),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok("/tmp/mock.bundle".into()),
        }
    }

    fn bundle_fetch(&self, _repo_path: &Path, bundle: &str) -> Result<String, VmBackendError> {
        let call = format!("bundle_fetch:{}", bundle);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Hash(h)) => Ok(h),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok("mock_fetched_hash".into()),
        }
    }

    fn run_container(&self, _image: &str, script: &str, _mounts: &[Mount]) -> Result<(i64, String), VmBackendError> {
        let call = format!("run_container:{}", &script[..script.len().min(50)]);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::ContainerOutput(code, out)) => Ok((code, out)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((0, String::new())),
        }
    }

    fn attach_container(&self, _image: &str, _env: &[(String, String)], _mounts: &[Mount]) -> Result<i64, VmBackendError> {
        let call = "attach_container".to_string();
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::ContainerExited(code)) => Ok(code),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(0),
        }
    }

    fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError> {
        let call = format!("prompt:{}", message);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Bool(b)) => Ok(b),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(true), // default: auto-confirm
        }
    }
}
