//! Shared git2 helpers used by both Git2Backend and RealBackend.
//!
//! Centralizes common operations to eliminate duplication.

use std::path::Path;
use git2::Repository;
use super::backend::VmBackendError;

/// Open a git repository at the given path.
pub fn open_repo(path: &Path) -> Result<Repository, VmBackendError> {
    Repository::open(path).map_err(|e| VmBackendError::Failed(format!("open {}: {}", path.display(), e)))
}

/// Find the commit hash for a given ref name, or None if not found.
pub fn find_commit_hash(repo: &Repository, ref_name: &str) -> Result<Option<String>, VmBackendError> {
    let reference = match repo.find_reference(ref_name) {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };
    let commit = reference.peel_to_commit()
        .map_err(|e| VmBackendError::Failed(e.to_string()))?;
    Ok(Some(commit.id().to_string()))
}

/// Create a signature for commits, falling back to defaults.
pub fn make_signature(repo: &Repository) -> git2::Signature<'static> {
    repo.signature().unwrap_or_else(|_| {
        git2::Signature::now("vm", "vm@local").unwrap()
    })
}

/// Write a ref (create or update) to point at the given hash.
pub fn write_ref(repo: &Repository, ref_name: &str, hash: &str) -> Result<(), VmBackendError> {
    let oid = git2::Oid::from_str(hash)
        .map_err(|e| VmBackendError::Failed(e.to_string()))?;
    repo.reference(ref_name, oid, true, "vm: ref_write")
        .map_err(|e| VmBackendError::Failed(e.to_string()))?;
    Ok(())
}

/// Compare two commit trees, returning (identical, files_changed).
pub fn compare_trees(repo: &Repository, a: &str, b: &str) -> Result<(bool, u32), VmBackendError> {
    let oid_a = git2::Oid::from_str(a).map_err(|e| VmBackendError::Failed(e.to_string()))?;
    let oid_b = git2::Oid::from_str(b).map_err(|e| VmBackendError::Failed(e.to_string()))?;
    let tree_a = repo.find_commit(oid_a).map_err(|e| VmBackendError::Failed(e.to_string()))?
        .tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;
    let tree_b = repo.find_commit(oid_b).map_err(|e| VmBackendError::Failed(e.to_string()))?
        .tree().map_err(|e| VmBackendError::Failed(e.to_string()))?;

    if tree_a.id() == tree_b.id() {
        Ok((true, 0))
    } else {
        let diff = repo.diff_tree_to_tree(Some(&tree_a), Some(&tree_b), None)
            .map_err(|e| VmBackendError::Failed(e.to_string()))?;
        let stats = diff.stats().map_err(|e| VmBackendError::Failed(e.to_string()))?;
        Ok((false, stats.files_changed() as u32))
    }
}

/// Checkout a ref, resetting the worktree.
pub fn checkout_ref(repo: &Repository, ref_name: &str) -> Result<(), VmBackendError> {
    repo.set_head(ref_name)
        .map_err(|e| VmBackendError::Failed(e.to_string()))?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| VmBackendError::Failed(e.to_string()))?;
    Ok(())
}

/// Create a commit with the given tree, parents, and message.
pub fn create_commit(
    repo: &Repository,
    tree_hash: &str,
    parents: &[String],
    message: &str,
    sig: &git2::Signature,
) -> Result<String, VmBackendError> {
    let tree_oid = git2::Oid::from_str(tree_hash)
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

    let oid = repo.commit(Some("HEAD"), sig, sig, message, &tree_obj, &parent_refs)
        .map_err(|e| VmBackendError::Failed(e.to_string()))?;
    Ok(oid.to_string())
}

/// Count commits between two oids (from exclusive, to inclusive).
pub fn count_between(repo: &Repository, from: git2::Oid, to: git2::Oid) -> u32 {
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(_) => return 0,
    };
    let _ = revwalk.push(to);
    let _ = revwalk.hide(from);
    revwalk.count() as u32
}
