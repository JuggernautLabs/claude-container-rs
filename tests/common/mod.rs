//! Shared test helpers — git repo creation, commit, branch, assertion utilities.
//!
//! Used by: vm_test, adversarial_test, end_to_end_vm_test, two_leg_test, property_test.
//!
//! Import with: `mod common; use common::*;`

#![allow(dead_code)]

use git2::Repository;
use std::path::{Path, PathBuf};

// ============================================================================
// Repo creation
// ============================================================================

/// Create a temp git repo with an initial commit on `main`.
/// Returns (TempDir guard, path to repo).
pub fn make_repo(name: &str) -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join(name);
    std::fs::create_dir_all(&path).unwrap();
    let repo = Repository::init(&path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    std::fs::write(path.join("README.md"), "# test\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    repo.commit(Some("refs/heads/main"), &sig, &sig, "initial", &tree, &[]).unwrap();
    repo.set_head("refs/heads/main").unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    (tmp, path)
}

// ============================================================================
// Git operations
// ============================================================================

/// Add a file and commit it, returning the commit hash as a String.
pub fn commit_file(path: &Path, file: &str, content: &str, msg: &str) -> String {
    let repo = Repository::open(path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    if let Some(parent) = Path::new(file).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(path.join(parent)).unwrap();
        }
    }
    std::fs::write(path.join(file), content).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(file)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent]).unwrap().to_string()
}

/// Add a file and commit, returning the Oid (for two_leg_test compatibility).
pub fn add_commit(path: &Path, file: &str, content: &str, msg: &str) -> git2::Oid {
    let repo = Repository::open(path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    if let Some(parent) = Path::new(file).parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(path.join(parent)).unwrap();
        }
    }
    std::fs::write(path.join(file), content).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new(file)).unwrap();
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &[&parent]).unwrap()
}

/// Create a branch at current HEAD.
pub fn git_branch(path: &Path, name: &str) {
    let repo = Repository::open(path).unwrap();
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch(name, &head, false).unwrap();
}

/// Switch to a branch (set HEAD + force checkout).
pub fn git_switch(path: &Path, name: &str) {
    let repo = Repository::open(path).unwrap();
    repo.set_head(&format!("refs/heads/{}", name)).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
}

/// Get the HEAD commit hash for a branch (as String).
pub fn head_of(path: &Path, name: &str) -> String {
    let repo = Repository::open(path).unwrap();
    let reference = repo.find_reference(&format!("refs/heads/{}", name)).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    commit.id().to_string()
}

/// Get the HEAD Oid for a branch (for two_leg_test compatibility).
pub fn branch_head(path: &Path, name: &str) -> git2::Oid {
    let repo = Repository::open(path).unwrap();
    let reference = repo.find_reference(&format!("refs/heads/{}", name)).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    commit.id()
}

/// Read a file's content from the worktree.
pub fn file_content(path: &Path, file: &str) -> String {
    std::fs::read_to_string(path.join(file)).unwrap_or_default()
}

// ============================================================================
// Assertions
// ============================================================================

/// Assert no conflict markers in any blob on the given branch.
pub fn assert_no_markers(path: &Path, branch: &str) {
    let repo = Repository::open(path).unwrap();
    let reference = repo.find_reference(&format!("refs/heads/{}", branch)).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    let tree = commit.tree().unwrap();
    tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if let Some(git2::ObjectType::Blob) = entry.kind() {
            let blob = repo.find_blob(entry.id()).unwrap();
            let content = std::str::from_utf8(blob.content()).unwrap_or("");
            let full = if dir.is_empty() { entry.name().unwrap_or("?").to_string() }
                       else { format!("{}{}", dir, entry.name().unwrap_or("?")) };
            assert!(!content.contains("<<<<<<<"), "markers in {} on {}", full, branch);
            assert!(!content.contains(">>>>>>>"), "markers in {} on {}", full, branch);
        }
        git2::TreeWalkResult::Ok
    }).unwrap();
}

/// Assert worktree has no uncommitted changes.
pub fn assert_worktree_clean(path: &Path) {
    let repo = Repository::open(path).unwrap();
    let statuses = repo.statuses(Some(
        git2::StatusOptions::new()
            .include_untracked(false)
            .include_ignored(false)
    )).unwrap();
    assert!(statuses.is_empty(),
        "worktree should be clean, but found {} dirty entries", statuses.len());
}

/// Check if a file exists in the tree of a branch.
pub fn tree_has_file(path: &Path, branch: &str, file: &str) -> bool {
    let repo = Repository::open(path).unwrap();
    let refname = format!("refs/heads/{}", branch);
    let reference = repo.find_reference(&refname).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    let tree = commit.tree().unwrap();
    let result = tree.get_name(file).is_some();
    result
}

/// Count commits on a branch (walk full history).
pub fn count_commits(path: &Path, branch: &str) -> usize {
    let repo = Repository::open(path).unwrap();
    let commit = repo.find_reference(&format!("refs/heads/{}", branch))
        .unwrap().peel_to_commit().unwrap();
    let mut count = 0;
    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push(commit.id()).unwrap();
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL).unwrap();
    for _ in revwalk {
        count += 1;
    }
    count
}
