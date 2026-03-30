//! Primitive-level backend trait — one method per op.
//!
//! The VM dispatches each Op to the corresponding backend method.
//! MockBackend records typed calls and returns canned responses.

use std::path::{Path, PathBuf};
use super::ops::Mount;

/// Errors from backend execution.
#[derive(Debug)]
pub enum VmBackendError {
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
pub trait VmBackend: Send + Sync {
    async fn ref_read(&self, repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError>;
    async fn ref_write(&self, repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError>;
    async fn tree_compare(&self, repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError>;
    async fn ancestry_check(&self, repo_path: &Path, a: &str, b: &str) -> Result<super::AncestryResult, VmBackendError>;
    async fn merge_trees(&self, repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError>;
    async fn checkout(&self, repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError>;
    async fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], message: &str) -> Result<String, VmBackendError>;
    async fn bundle_create(&self, session: &str, repo: &str) -> Result<String, VmBackendError>;
    async fn bundle_fetch(&self, repo_path: &Path, bundle_path: &str) -> Result<String, VmBackendError>;
    async fn run_container(&self, image: &str, script: &str, mounts: &[Mount]) -> Result<(i64, String), VmBackendError>;

    // ── High-level sync ops ──
    async fn extract(&self, session: &str, repo: &str, host_path: &Path, session_branch: &str) -> Result<(u32, String), VmBackendError>;
    async fn inject(&self, session: &str, repo: &str, host_path: &Path, branch: &str) -> Result<(), VmBackendError>;
    async fn force_inject(&self, session: &str, repo: &str, host_path: &Path, branch: &str) -> Result<(), VmBackendError>;

    // ── Agent/Human ──
    async fn agent_run(&self, task: &super::AgentTask, context: &str, mounts: &[Mount]) -> Result<(bool, Option<String>, Option<String>), VmBackendError>;
    async fn interactive_session(&self, prompt: Option<&str>, mounts: &[Mount]) -> Result<i64, VmBackendError>;
    async fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError>;
}

// ============================================================================
// Typed call recording — no strings
// ============================================================================

/// A recorded backend call with typed fields.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedCall {
    RefRead { repo_path: PathBuf, ref_name: String },
    RefWrite { repo_path: PathBuf, ref_name: String, hash: String },
    TreeCompare { repo_path: PathBuf, a: String, b: String },
    AncestryCheck { repo_path: PathBuf, a: String, b: String },
    MergeTrees { repo_path: PathBuf, ours: String, theirs: String },
    Checkout { repo_path: PathBuf, ref_name: String },
    Commit { repo_path: PathBuf, tree: String, parents: Vec<String>, message: String },
    BundleCreate { session: String, repo: String },
    BundleFetch { repo_path: PathBuf, bundle_path: String },
    RunContainer { image: String },
    Extract { session: String, repo: String, host_path: PathBuf, session_branch: String },
    Inject { session: String, repo: String, host_path: PathBuf, branch: String },
    ForceInject { session: String, repo: String, host_path: PathBuf, branch: String },
    AgentRun { task_debug: String },
    InteractiveSession { prompt: Option<String> },
    PromptUser { message: String },
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

/// Match a recorded call by variant.
#[derive(Debug, Clone)]
pub enum CallMatcher {
    RefRead,
    RefWrite,
    TreeCompare,
    AncestryCheck,
    MergeTrees,
    Checkout,
    Commit,
    BundleCreate,
    BundleFetch,
    RunContainer,
    Extract,
    Inject,
    ForceInject,
    AgentRun,
    InteractiveSession,
    PromptUser,
}

impl CallMatcher {
    fn matches(&self, call: &RecordedCall) -> bool {
        matches!(
            (self, call),
            (CallMatcher::RefRead, RecordedCall::RefRead { .. }) |
            (CallMatcher::RefWrite, RecordedCall::RefWrite { .. }) |
            (CallMatcher::TreeCompare, RecordedCall::TreeCompare { .. }) |
            (CallMatcher::AncestryCheck, RecordedCall::AncestryCheck { .. }) |
            (CallMatcher::MergeTrees, RecordedCall::MergeTrees { .. }) |
            (CallMatcher::Checkout, RecordedCall::Checkout { .. }) |
            (CallMatcher::Commit, RecordedCall::Commit { .. }) |
            (CallMatcher::BundleCreate, RecordedCall::BundleCreate { .. }) |
            (CallMatcher::BundleFetch, RecordedCall::BundleFetch { .. }) |
            (CallMatcher::RunContainer, RecordedCall::RunContainer { .. }) |
            (CallMatcher::Extract, RecordedCall::Extract { .. }) |
            (CallMatcher::Inject, RecordedCall::Inject { .. }) |
            (CallMatcher::ForceInject, RecordedCall::ForceInject { .. }) |
            (CallMatcher::AgentRun, RecordedCall::AgentRun { .. }) |
            (CallMatcher::InteractiveSession, RecordedCall::InteractiveSession { .. }) |
            (CallMatcher::PromptUser, RecordedCall::PromptUser { .. })
        )
    }
}

// ============================================================================
// MockBackend — lenient, pattern-matching, typed recording
// ============================================================================

struct MockEntry {
    matcher: CallMatcher,
    result: MockResult,
}

/// Mock backend for unit tests.
/// Records typed calls. Returns canned responses matched by call variant.
pub struct MockBackend {
    responses: std::sync::Mutex<Vec<MockEntry>>,
    calls: std::sync::Mutex<Vec<RecordedCall>>,
}

impl std::fmt::Debug for MockBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockBackend")
            .field("calls", &self.calls.lock().unwrap().len())
            .finish()
    }
}

impl Default for MockBackend {
    fn default() -> Self {
        Self {
            responses: std::sync::Mutex::new(Vec::new()),
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }
}

impl MockBackend {
    pub fn new() -> Self { Self::default() }

    /// Add a canned response for a call variant.
    pub fn on(&self, matcher: CallMatcher, result: MockResult) {
        self.responses.lock().unwrap().push(MockEntry { matcher, result });
    }

    /// Get all recorded calls.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls.lock().unwrap().clone()
    }

    /// Check if a specific call variant was recorded.
    pub fn was_called(&self, matcher: &CallMatcher) -> bool {
        self.calls.lock().unwrap().iter().any(|c| matcher.matches(c))
    }

    /// Count how many times a call variant was recorded.
    pub fn call_count(&self, matcher: &CallMatcher) -> usize {
        self.calls.lock().unwrap().iter().filter(|c| matcher.matches(c)).count()
    }

    fn record(&self, call: RecordedCall) {
        self.calls.lock().unwrap().push(call);
    }

    fn pop_response(&self, call: &RecordedCall) -> Option<MockResult> {
        let mut responses = self.responses.lock().unwrap();
        if let Some(idx) = responses.iter().position(|e| e.matcher.matches(call)) {
            Some(responses.remove(idx).result)
        } else {
            None
        }
    }

    fn dispatch(&self, call: RecordedCall) -> Option<MockResult> {
        let result = self.pop_response(&call);
        self.record(call);
        result
    }
}

impl VmBackend for MockBackend {
    async fn ref_read(&self, repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError> {
        let call = RecordedCall::RefRead { repo_path: repo_path.into(), ref_name: ref_name.into() };
        match self.dispatch(call) {
            Some(MockResult::Hash(h)) => Ok(Some(h)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(None),
        }
    }

    async fn ref_write(&self, repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
        let call = RecordedCall::RefWrite { repo_path: repo_path.into(), ref_name: ref_name.into(), hash: hash.into() };
        match self.dispatch(call) {
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    async fn tree_compare(&self, repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
        let call = RecordedCall::TreeCompare { repo_path: repo_path.into(), a: a.into(), b: b.into() };
        match self.dispatch(call) {
            Some(MockResult::Comparison(identical, files)) => Ok((identical, files)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, 0)),
        }
    }

    async fn ancestry_check(&self, repo_path: &Path, a: &str, b: &str) -> Result<super::AncestryResult, VmBackendError> {
        let call = RecordedCall::AncestryCheck { repo_path: repo_path.into(), a: a.into(), b: b.into() };
        match self.dispatch(call) {
            Some(MockResult::Ancestry(r)) => Ok(r),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(super::AncestryResult::Unknown),
        }
    }

    async fn merge_trees(&self, repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError> {
        let call = RecordedCall::MergeTrees { repo_path: repo_path.into(), ours: ours.into(), theirs: theirs.into() };
        match self.dispatch(call) {
            Some(MockResult::MergeClean(tree)) => Ok((true, Some(tree), vec![])),
            Some(MockResult::MergeConflict(files)) => Ok((false, None, files)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, Some("mock_tree".into()), vec![])),
        }
    }

    async fn checkout(&self, repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError> {
        let call = RecordedCall::Checkout { repo_path: repo_path.into(), ref_name: ref_name.into() };
        match self.dispatch(call) {
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    async fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], msg: &str) -> Result<String, VmBackendError> {
        let call = RecordedCall::Commit { repo_path: repo_path.into(), tree: tree.into(), parents: parents.to_vec(), message: msg.into() };
        match self.dispatch(call) {
            Some(MockResult::Hash(h)) => Ok(h),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok("mock_commit_hash".into()),
        }
    }

    async fn bundle_create(&self, session: &str, repo: &str) -> Result<String, VmBackendError> {
        let call = RecordedCall::BundleCreate { session: session.into(), repo: repo.into() };
        match self.dispatch(call) {
            Some(MockResult::Hash(h)) => Ok(h),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok("/tmp/mock.bundle".into()),
        }
    }

    async fn bundle_fetch(&self, repo_path: &Path, bundle: &str) -> Result<String, VmBackendError> {
        let call = RecordedCall::BundleFetch { repo_path: repo_path.into(), bundle_path: bundle.into() };
        match self.dispatch(call) {
            Some(MockResult::Hash(h)) => Ok(h),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok("mock_fetched_hash".into()),
        }
    }

    async fn run_container(&self, image: &str, _script: &str, _mounts: &[Mount]) -> Result<(i64, String), VmBackendError> {
        let call = RecordedCall::RunContainer { image: image.into() };
        match self.dispatch(call) {
            Some(MockResult::ContainerOutput(code, out)) => Ok((code, out)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((0, String::new())),
        }
    }

    async fn extract(&self, session: &str, repo: &str, host_path: &Path, session_branch: &str) -> Result<(u32, String), VmBackendError> {
        let call = RecordedCall::Extract { session: session.into(), repo: repo.into(), host_path: host_path.into(), session_branch: session_branch.into() };
        match self.dispatch(call) {
            Some(MockResult::Hash(h)) => Ok((1, h)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((1, "mock_extracted_head".into())),
        }
    }

    async fn inject(&self, session: &str, repo: &str, host_path: &Path, branch: &str) -> Result<(), VmBackendError> {
        let call = RecordedCall::Inject { session: session.into(), repo: repo.into(), host_path: host_path.into(), branch: branch.into() };
        match self.dispatch(call) {
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    async fn force_inject(&self, session: &str, repo: &str, host_path: &Path, branch: &str) -> Result<(), VmBackendError> {
        let call = RecordedCall::ForceInject { session: session.into(), repo: repo.into(), host_path: host_path.into(), branch: branch.into() };
        match self.dispatch(call) {
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(()),
        }
    }

    async fn agent_run(&self, task: &super::AgentTask, _context: &str, _mounts: &[Mount]) -> Result<(bool, Option<String>, Option<String>), VmBackendError> {
        let call = RecordedCall::AgentRun { task_debug: format!("{:?}", task) };
        match self.dispatch(call) {
            Some(MockResult::Hash(h)) => Ok((true, Some("resolved".into()), Some(h))),
            Some(MockResult::Bool(false)) => Ok((false, None, None)),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok((true, Some("mock resolved".into()), Some("mock_agent_head".into()))),
        }
    }

    async fn interactive_session(&self, prompt: Option<&str>, _mounts: &[Mount]) -> Result<i64, VmBackendError> {
        let call = RecordedCall::InteractiveSession { prompt: prompt.map(|s| s.into()) };
        match self.dispatch(call) {
            Some(MockResult::ContainerExited(code)) => Ok(code),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(0),
        }
    }

    async fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError> {
        let call = RecordedCall::PromptUser { message: message.into() };
        match self.dispatch(call) {
            Some(MockResult::Bool(b)) => Ok(b),
            Some(MockResult::Error(e)) => Err(VmBackendError::Failed(e)),
            _ => Ok(true),
        }
    }
}
