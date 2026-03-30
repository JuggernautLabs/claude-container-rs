//! Staging backend — runs real ops but writes refs to staging namespace.
//!
//! All git objects (trees, commits, blobs) are created for real — they're
//! immutable and harmless. Only ref writes go to `refs/gitvm/staging/...`
//! instead of the real branch refs.
//!
//! On confirm: fast-forward real refs to staged values.
//! On decline: delete staging refs. Objects are garbage collected.
//!
//! Container ops (inject, extract) run against staging refs too where
//! possible. Extract writes to a staging session branch. Inject must
//! run for real (modifies the container volume) but the re-extract
//! after inject writes to staging.

use std::path::Path;
use std::collections::BTreeMap;
use git2::Repository;
use super::backend::{VmBackend, VmBackendError};
use super::ops::{Mount, AgentTask, AncestryResult};

const STAGING_PREFIX: &str = "refs/gitvm/staging";

/// A ref that was staged (old value → new value).
#[derive(Debug, Clone)]
pub struct StagedRef {
    pub repo_path: std::path::PathBuf,
    pub real_ref: String,
    pub staging_ref: String,
    pub old_hash: Option<String>,
    pub new_hash: String,
}

/// Wraps any VmBackend and intercepts ref_write to stage instead of commit.
/// Merge/commit/checkout ops run for real against git objects.
/// ref_write goes to staging refs.
pub struct StagingBackend<B: VmBackend> {
    inner: B,
    staged_refs: std::sync::Mutex<Vec<StagedRef>>,
}

impl<B: VmBackend> StagingBackend<B> {
    pub fn new(inner: B) -> Self {
        Self {
            inner,
            staged_refs: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Get all staged refs (for preview).
    pub fn staged(&self) -> Vec<StagedRef> {
        self.staged_refs.lock().unwrap().clone()
    }

    /// Apply all staged refs — fast-forward real refs to staged values.
    pub fn apply(&self) -> Result<(), VmBackendError> {
        let staged = self.staged_refs.lock().unwrap().clone();
        for s in &staged {
            // Write real ref
            {
                let repo = Repository::open(&s.repo_path)
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;
                let oid = git2::Oid::from_str(&s.new_hash)
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;
                repo.reference(&s.real_ref, oid, true, "gitvm: apply staged")
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;
            }
            // Checkout if HEAD matches
            {
                let repo = Repository::open(&s.repo_path)
                    .map_err(|e| VmBackendError::Failed(e.to_string()))?;
                let head_ref_name = repo.head().ok()
                    .and_then(|h| h.name().map(|n| n.to_string()));
                if head_ref_name.as_deref() == Some(&s.real_ref) {
                    let _ = repo.checkout_head(Some(
                        git2::build::CheckoutBuilder::new().force()
                    ));
                }
            }
            // Clean up staging ref — write it to the same oid then delete
            delete_ref(&s.repo_path, &s.staging_ref);
        }
        Ok(())
    }

    /// Discard all staged refs — clean up without applying.
    pub fn discard(&self) {
        let staged = self.staged_refs.lock().unwrap().clone();
        for s in &staged {
            delete_ref(&s.repo_path, &s.staging_ref);
        }
        self.staged_refs.lock().unwrap().clear();
    }

    fn staging_ref_name(real_ref: &str) -> String {
        // refs/heads/main → refs/gitvm/staging/heads/main
        if let Some(suffix) = real_ref.strip_prefix("refs/") {
            format!("{}/{}", STAGING_PREFIX, suffix)
        } else {
            format!("{}/{}", STAGING_PREFIX, real_ref)
        }
    }
}

impl<B: VmBackend> VmBackend for StagingBackend<B> {
    // Read ops — pass through to inner (read real state)
    async fn ref_read(&self, repo_path: &Path, ref_name: &str) -> Result<Option<String>, VmBackendError> {
        // First check if we have a staged version
        let staged = self.staged_refs.lock().unwrap();
        for s in staged.iter().rev() {
            if s.repo_path == repo_path && s.real_ref == ref_name {
                return Ok(Some(s.new_hash.clone()));
            }
        }
        drop(staged);
        self.inner.ref_read(repo_path, ref_name).await
    }

    // Ref write — STAGE instead of writing to real ref
    async fn ref_write(&self, repo_path: &Path, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
        let staging_ref = Self::staging_ref_name(ref_name);

        // Read current real value
        let old_hash = self.inner.ref_read(repo_path, ref_name).await?;

        // Write to staging ref (real git object, staging namespace)
        let repo = Repository::open(repo_path)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let oid = git2::Oid::from_str(hash)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        repo.reference(&staging_ref, oid, true, "gitvm: staging")
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;

        // Record the staging
        self.staged_refs.lock().unwrap().push(StagedRef {
            repo_path: repo_path.to_path_buf(),
            real_ref: ref_name.to_string(),
            staging_ref,
            old_hash,
            new_hash: hash.to_string(),
        });

        Ok(())
    }

    // Tree ops — pass through (create real objects, they're immutable)
    async fn tree_compare(&self, repo_path: &Path, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
        self.inner.tree_compare(repo_path, a, b).await
    }
    async fn ancestry_check(&self, repo_path: &Path, a: &str, b: &str) -> Result<AncestryResult, VmBackendError> {
        self.inner.ancestry_check(repo_path, a, b).await
    }
    async fn merge_trees(&self, repo_path: &Path, ours: &str, theirs: &str) -> Result<(bool, Option<String>, Vec<String>), VmBackendError> {
        self.inner.merge_trees(repo_path, ours, theirs).await
    }
    async fn checkout(&self, repo_path: &Path, ref_name: &str) -> Result<(), VmBackendError> {
        // Checkout the staging ref if it exists, otherwise the real ref
        let staged = self.staged_refs.lock().unwrap();
        for s in staged.iter().rev() {
            if s.repo_path == repo_path && s.real_ref == ref_name {
                return self.inner.checkout(repo_path, &s.staging_ref).await;
            }
        }
        drop(staged);
        self.inner.checkout(repo_path, ref_name).await
    }
    async fn commit(&self, repo_path: &Path, tree: &str, parents: &[String], message: &str) -> Result<String, VmBackendError> {
        // Create real commit object WITHOUT updating HEAD.
        // The inner backend uses Some("HEAD") which would move the branch.
        // We create the object with None — it exists in the repo but no ref points to it.
        use super::git2_ops::{open_repo, make_signature};
        let repo = open_repo(repo_path)?;
        let sig = make_signature(&repo);
        let tree_oid = git2::Oid::from_str(tree)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let tree_obj = repo.find_tree(tree_oid)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let parent_commits: Vec<git2::Commit> = parents.iter()
            .map(|p| {
                let oid = git2::Oid::from_str(p).map_err(|e| VmBackendError::Failed(e.to_string()))?;
                repo.find_commit(oid).map_err(|e| VmBackendError::Failed(e.to_string()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let parent_refs: Vec<&git2::Commit> = parent_commits.iter().collect();
        // None = don't update any ref
        let oid = repo.commit(None, &sig, &sig, message, &tree_obj, &parent_refs)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok(oid.to_string())
    }

    // Transport — pass through
    async fn bundle_create(&self, session: &str, repo: &str) -> Result<String, VmBackendError> {
        self.inner.bundle_create(session, repo).await
    }
    async fn bundle_fetch(&self, repo_path: &Path, bundle_path: &str) -> Result<String, VmBackendError> {
        self.inner.bundle_fetch(repo_path, bundle_path).await
    }

    // Container ops — pass through (can't stage container modifications)
    async fn run_container(&self, image: &str, script: &str, mounts: &[Mount]) -> Result<(i64, String), VmBackendError> {
        self.inner.run_container(image, script, mounts).await
    }
    async fn extract(&self, session: &str, repo: &str, host_path: &Path, session_branch: &str) -> Result<(u32, String), VmBackendError> {
        // Extract to a staging session branch
        let staging_branch = format!("gitvm-staging-{}", session_branch);
        let result = self.inner.extract(session, repo, host_path, &staging_branch).await?;

        // Record the staging (session branch)
        let ref_name = format!("refs/heads/{}", session_branch);
        let staging_ref = format!("refs/heads/{}", staging_branch);
        self.staged_refs.lock().unwrap().push(StagedRef {
            repo_path: host_path.to_path_buf(),
            real_ref: ref_name,
            staging_ref,
            old_hash: None,
            new_hash: result.1.clone(),
        });

        Ok(result)
    }
    async fn inject(&self, session: &str, repo: &str, host_path: &Path, branch: &str) -> Result<(), VmBackendError> {
        // Inject can't be staged — it modifies the container volume
        self.inner.inject(session, repo, host_path, branch).await
    }
    async fn force_inject(&self, session: &str, repo: &str, host_path: &Path, branch: &str) -> Result<(), VmBackendError> {
        self.inner.force_inject(session, repo, host_path, branch).await
    }

    // Agent/Human — pass through
    async fn agent_run(&self, task: &AgentTask, context: &str, mounts: &[Mount]) -> Result<(bool, Option<String>, Option<String>), VmBackendError> {
        self.inner.agent_run(task, context, mounts).await
    }
    async fn interactive_session(&self, prompt: Option<&str>, mounts: &[Mount]) -> Result<i64, VmBackendError> {
        self.inner.interactive_session(prompt, mounts).await
    }
    async fn prompt_user(&self, message: &str) -> Result<bool, VmBackendError> {
        self.inner.prompt_user(message).await
    }
}

/// Delete a git ref by name.
fn delete_ref(repo_path: &Path, ref_name: &str) {
    if let Ok(repo) = Repository::open(repo_path) {
        let _ = repo.find_reference(ref_name).map(|mut r| r.delete());
    }
}
