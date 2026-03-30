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

    // ── Agent/Human ops ──
    /// Run an agent (Claude) with a specific task. Returns whether the agent
    /// resolved the task, an optional description, and the new container HEAD.
    fn agent_run(&self, task: &super::AgentTask, context: &str, mounts: &[Mount])
        -> Result<(bool, Option<String>, Option<String>), VmBackendError>;
    /// Drop a human into an interactive session. Returns exit code.
    fn interactive_session(&self, prompt: Option<&str>, mounts: &[Mount])
        -> Result<i64, VmBackendError>;

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

    fn agent_run(&self, task: &super::AgentTask, _context: &str, _mounts: &[Mount])
        -> Result<(bool, Option<String>, Option<String>), VmBackendError> {
        let call = format!("agent_run:{:?}", task);
        self.record(&call);
        match self.pop_response(&call) {
            Some(MockResult::Hash(h)) => Ok((true, Some("resolved".into()), Some(h))),
            Some(MockResult::Bool(false)) => Ok((false, None, None)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, Some("mock resolved".into()), Some("mock_agent_head".into()))),
        }
    }

    fn interactive_session(&self, prompt: Option<&str>, _mounts: &[Mount])
        -> Result<i64, VmBackendError> {
        let call = format!("interactive_session:{}", prompt.unwrap_or("none"));
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

// ============================================================================
// StrictMockBackend — ordered expectations, panics on mismatch
// ============================================================================

/// A strict mock that verifies calls happen in exact order with exact patterns.
/// Panics on:
/// - Unexpected call (no matching expectation)
/// - Call out of order
/// - Unconsumed expectations (checked via `assert_complete()`)
#[derive(Debug)]
pub struct StrictMockBackend {
    expectations: std::sync::Mutex<Vec<Expectation>>,
    cursor: std::sync::Mutex<usize>,
    calls: std::sync::Mutex<Vec<String>>,
}

/// One expected call.
#[derive(Debug, Clone)]
pub struct Expectation {
    /// Pattern to match against the call description
    pub pattern: String,
    /// Result to return
    pub result: MockResult,
}

impl StrictMockBackend {
    pub fn new() -> Self {
        Self {
            expectations: std::sync::Mutex::new(Vec::new()),
            cursor: std::sync::Mutex::new(0),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Add the next expected call (order matters).
    pub fn expect(&self, pattern: &str, result: MockResult) {
        self.expectations.lock().unwrap().push(Expectation {
            pattern: pattern.to_string(),
            result,
        });
    }

    /// Assert all expectations were consumed. Panics with details if not.
    pub fn assert_complete(&self) {
        let expectations = self.expectations.lock().unwrap();
        let cursor = *self.cursor.lock().unwrap();
        let calls = self.calls.lock().unwrap();
        if cursor < expectations.len() {
            let remaining: Vec<_> = expectations[cursor..].iter()
                .map(|e| e.pattern.clone())
                .collect();
            panic!(
                "StrictMock: {} unconsumed expectation(s): {:?}\nCalls made: {:?}",
                remaining.len(), remaining, *calls,
            );
        }
    }

    /// Get recorded calls.
    pub fn recorded_calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn next_response(&self, call: &str) -> MockResult {
        self.calls.lock().unwrap().push(call.to_string());

        let expectations = self.expectations.lock().unwrap();
        let mut cursor = self.cursor.lock().unwrap();

        if *cursor >= expectations.len() {
            panic!(
                "StrictMock: unexpected call '{}' (no more expectations)\nAll calls: {:?}",
                call, self.calls.lock().unwrap(),
            );
        }

        let expected = &expectations[*cursor];
        if !call.contains(&expected.pattern) {
            panic!(
                "StrictMock: call #{} mismatch\n  expected pattern: '{}'\n  actual call:      '{}'\nAll calls so far: {:?}",
                *cursor, expected.pattern, call, self.calls.lock().unwrap(),
            );
        }

        let result = expected.result.clone();
        *cursor += 1;
        result
    }
}

impl VmBackend for StrictMockBackend {
    fn ref_read(&self, _repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError> {
        match self.next_response(&format!("ref_read:{}", ref_name)) {
            MockResult::Hash(h) => Ok(Some(h)),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok(None),
        }
    }

    fn ref_write(&self, _repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
        match self.next_response(&format!("ref_write:{}={}", ref_name, hash)) {
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    fn tree_compare(&self, _repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
        match self.next_response(&format!("tree_compare:{}..{}", a, b)) {
            MockResult::Comparison(identical, files) => Ok((identical, files)),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, 0)),
        }
    }

    fn ancestry_check(&self, _repo_path: &Path, a: &str, b: &str) -> Result<super::AncestryResult, VmBackendError> {
        match self.next_response(&format!("ancestry_check:{}..{}", a, b)) {
            MockResult::Ancestry(r) => Ok(r),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok(super::AncestryResult::Unknown),
        }
    }

    fn merge_trees(&self, _repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError> {
        match self.next_response(&format!("merge_trees:{}+{}", ours, theirs)) {
            MockResult::MergeClean(tree) => Ok((true, Some(tree), vec![])),
            MockResult::MergeConflict(files) => Ok((false, None, files)),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, Some("mock_tree".into()), vec![])),
        }
    }

    fn checkout(&self, _repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError> {
        match self.next_response(&format!("checkout:{}", ref_name)) {
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    fn commit(&self, _repo_path: &Path, _tree: &str, _parents: &[String], msg: &str) -> Result<String, VmBackendError> {
        match self.next_response(&format!("commit:{}", msg)) {
            MockResult::Hash(h) => Ok(h),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok("strict_mock_commit".into()),
        }
    }

    fn bundle_create(&self, _session: &str, repo: &str) -> Result<String, VmBackendError> {
        match self.next_response(&format!("bundle_create:{}", repo)) {
            MockResult::Hash(h) => Ok(h),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok("/tmp/strict.bundle".into()),
        }
    }

    fn bundle_fetch(&self, _repo_path: &Path, bundle: &str) -> Result<String, VmBackendError> {
        match self.next_response(&format!("bundle_fetch:{}", bundle)) {
            MockResult::Hash(h) => Ok(h),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok("strict_fetched".into()),
        }
    }

    fn run_container(&self, _image: &str, script: &str, _mounts: &[Mount]) -> Result<(i64, String), VmBackendError> {
        let short = &script[..script.len().min(50)];
        match self.next_response(&format!("run_container:{}", short)) {
            MockResult::ContainerOutput(code, out) => Ok((code, out)),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok((0, String::new())),
        }
    }

    fn agent_run(&self, task: &super::AgentTask, _context: &str, _mounts: &[Mount])
        -> Result<(bool, Option<String>, Option<String>), VmBackendError> {
        match self.next_response(&format!("agent_run:{:?}", task)) {
            MockResult::Hash(h) => Ok((true, Some("resolved".into()), Some(h))),
            MockResult::Bool(false) => Ok((false, None, None)),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, Some("strict resolved".into()), Some("strict_head".into()))),
        }
    }

    fn interactive_session(&self, prompt: Option<&str>, _mounts: &[Mount])
        -> Result<i64, VmBackendError> {
        match self.next_response(&format!("interactive_session:{}", prompt.unwrap_or("none"))) {
            MockResult::ContainerExited(code) => Ok(code),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok(0),
        }
    }

    fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError> {
        match self.next_response(&format!("prompt:{}", message)) {
            MockResult::Bool(b) => Ok(b),
            MockResult::Error(e) => Err(VmBackendError::Failed(e)),
            _ => Ok(true),
        }
    }
}
