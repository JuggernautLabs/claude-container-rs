//! End-to-end sync tests — extract, merge, inject on real repos and Docker volumes.
//! These verify the ACTUAL git operations, not just decision logic.
//!
//! Run: cargo test --test sync_e2e_test -- --ignored --nocapture --test-threads=1

mod harness;
use harness::*;
use git2::Repository;
use std::path::Path;

// ============================================================================
// 1. Extract creates session branch with correct name
// ============================================================================

#[tokio::test]
#[ignore]
async fn extract_creates_session_branch_on_host() {
    let session = TestSession::new("sync-extract").await;
    let repo = TestRepo::new("extract-branch");

    // Put a repo in the session volume
    let setup = session.run_simple(
        BASE_IMAGE,
        &format!(
            "cd /workspace && git clone /upstream test-repo && cd test-repo && \
             git config user.email 'test@test.com' && git config user.name 'test' && \
             echo 'container work' > container.txt && git add . && git commit -m 'container commit'",
        ),
    ).await;
    // Clone needs the host repo mounted — use run_container with binds
    let tc = session.run_container(
        BASE_IMAGE,
        &format!(
            "git config --global --add safe.directory '*' && \
             git clone /upstream /workspace/test-repo && \
             cd /workspace/test-repo && \
             git config user.email 'test@test.com' && git config user.name 'test' && \
             echo 'container work' > container.txt && git add . && git commit -m 'container commit' && \
             git rev-parse HEAD"
        ),
        vec![],
        vec![
            format!("{}:/workspace", session.session_volume()),
            format!("{}:/upstream:ro", repo.path.display()),
        ],
    ).await;
    let result = tc.wait_and_collect().await;
    result.assert_success();
    let container_head = result.stdout.trim().to_string();
    assert!(!container_head.is_empty(), "Should have a HEAD");

    // Now extract using the sync engine
    let d = docker();
    let engine = gitvm::sync::SyncEngine::new(d);
    let session_name = gitvm::types::SessionName::new(&session.name);
    let session_branch = session_name.to_string();

    let extract_result = engine.extract(
        &session_name,
        "test-repo",
        &repo.path,
        &session_branch,
    ).await;

    assert!(extract_result.is_ok(), "Extract should succeed: {:?}", extract_result.err());

    // THE KEY ASSERTION: session branch exists on host with correct name
    let host_repo = Repository::open(&repo.path).unwrap();
    let branch = host_repo.find_branch(&session_branch, git2::BranchType::Local);
    assert!(branch.is_ok(), "Session branch '{}' should exist on host. Branches: {:?}",
        session_branch,
        host_repo.branches(Some(git2::BranchType::Local)).unwrap()
            .filter_map(|b| b.ok())
            .filter_map(|(b, _)| b.name().ok().flatten().map(|s| s.to_string()))
            .collect::<Vec<_>>()
    );

    // Branch should point to the container's HEAD
    let branch_head = branch.unwrap().get().peel_to_commit().unwrap().id().to_string();
    assert_eq!(branch_head, container_head,
        "Session branch should point to container HEAD");
}

// ============================================================================
// 2. Merge actually commits to target branch
// ============================================================================

#[test]
fn merge_squash_creates_commit_on_target() {
    let repo = TestRepo::new("merge-target");
    let main_before = repo.head();

    // Create a session branch with new work
    let git_repo = Repository::open(&repo.path).unwrap();
    let head = git_repo.head().unwrap().peel_to_commit().unwrap();
    git_repo.branch("test-session", &head, false).unwrap();
    git_repo.set_head("refs/heads/test-session").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    // Add commits on session branch
    repo.commit("session work 1", &[("session1.txt", "work1")]);
    repo.commit("session work 2", &[("session2.txt", "work2")]);
    let session_head = repo.head();

    // Switch back to main
    let main_branch = git_repo.find_branch("master", git2::BranchType::Local)
        .or_else(|_| git_repo.find_branch("main", git2::BranchType::Local))
        .unwrap();
    git_repo.set_head(main_branch.get().name().unwrap()).unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    let main_name = main_branch.name().unwrap().unwrap().to_string();

    // Merge session → main (squash)
    let d = docker();
    let engine = gitvm::sync::SyncEngine::new(d);
    let result = engine.merge(&repo.path, "test-session", &main_name, true);

    assert!(result.is_ok(), "Merge should succeed: {:?}", result.err());
    let outcome = result.unwrap();

    // THE KEY ASSERTION: main moved forward
    let main_after = repo.head();
    assert_ne!(main_before, main_after, "Main should have a new commit after merge");

    // Squash merge = single parent
    let new_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(new_commit.parent_count(), 1, "Squash merge should have 1 parent");

    // The session files should exist on main now
    assert!(repo.path.join("session1.txt").exists(), "session1.txt should be on main");
    assert!(repo.path.join("session2.txt").exists(), "session2.txt should be on main");
}

// ============================================================================
// 3. Trial merge detects real conflicts
// ============================================================================

#[test]
fn trial_merge_detects_real_conflict() {
    let repo = TestRepo::new("trial-conflict");

    // Create diverged branches that conflict
    let git_repo = Repository::open(&repo.path).unwrap();
    let head = git_repo.head().unwrap().peel_to_commit().unwrap();

    // Branch A: edit README.md
    git_repo.branch("branch-a", &head, false).unwrap();
    git_repo.set_head("refs/heads/branch-a").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    repo.commit("branch-a edit", &[("README.md", "branch A content\n")]);
    let a_head = repo.head();

    // Branch B: edit same file differently
    git_repo.set_head("refs/heads/master").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    // Need to re-find head after checkout
    let master_head = git_repo.head().unwrap().peel_to_commit().unwrap();
    git_repo.branch("branch-b", &master_head, false).unwrap();
    git_repo.set_head("refs/heads/branch-b").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    repo.commit("branch-b edit", &[("README.md", "branch B content\n")]);
    let b_head = repo.head();

    // Trial merge: A into B
    let d = docker();
    let engine = gitvm::sync::SyncEngine::new(d);
    let a_hash = gitvm::types::CommitHash::new(a_head);
    let b_hash = gitvm::types::CommitHash::new(b_head);
    let result = engine.trial_merge(&repo.path, &b_hash, &a_hash);

    assert!(result.is_some(), "Trial merge should return a result");
    let conflict_files = result.unwrap();
    assert!(!conflict_files.is_empty(), "Should detect conflict");
    assert!(conflict_files.contains(&"README.md".to_string()),
        "README.md should be in conflicts. Got: {:?}", conflict_files);
}

#[test]
fn trial_merge_clean_returns_empty() {
    let repo = TestRepo::new("trial-clean");

    // Create diverged branches that DON'T conflict
    let git_repo = Repository::open(&repo.path).unwrap();
    let head = git_repo.head().unwrap().peel_to_commit().unwrap();

    git_repo.branch("branch-a", &head, false).unwrap();
    git_repo.set_head("refs/heads/branch-a").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    repo.commit("add file A", &[("a.txt", "aaa")]);
    let a_head = repo.head();

    git_repo.set_head("refs/heads/master").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    let master_head = git_repo.head().unwrap().peel_to_commit().unwrap();
    git_repo.branch("branch-b", &master_head, false).unwrap();
    git_repo.set_head("refs/heads/branch-b").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    repo.commit("add file B", &[("b.txt", "bbb")]);
    let b_head = repo.head();

    let d = docker();
    let engine = gitvm::sync::SyncEngine::new(d);
    let result = engine.trial_merge(
        &repo.path,
        &gitvm::types::CommitHash::new(b_head),
        &gitvm::types::CommitHash::new(a_head),
    );

    assert!(result.is_some(), "Should return result");
    assert!(result.unwrap().is_empty(), "Clean merge should have no conflicts");
}

// ============================================================================
// 4. Role filtering excludes dependencies
// ============================================================================

#[test]
fn project_repos_excludes_dependencies() {
    use gitvm::types::config::*;
    use std::collections::BTreeMap;

    let mut projects = BTreeMap::new();
    projects.insert("app".to_string(), ProjectConfig {
        path: "/tmp/app".into(), main: true, role: RepoRole::Project,
    });
    projects.insert("lib".to_string(), ProjectConfig {
        path: "/tmp/lib".into(), main: false, role: RepoRole::Project,
    });
    projects.insert("vendor".to_string(), ProjectConfig {
        path: "/tmp/vendor".into(), main: false, role: RepoRole::Dependency,
    });

    let config = SessionConfig { version: Some("1".into()), projects };

    let project_repos = config.project_repos();
    assert_eq!(project_repos.len(), 2, "Should have 2 project repos");
    assert!(project_repos.contains_key("app"));
    assert!(project_repos.contains_key("lib"));
    assert!(!project_repos.contains_key("vendor"), "Vendor is a dependency, not a project");

    let dep_repos = config.dependency_repos();
    assert_eq!(dep_repos.len(), 1);
    assert!(dep_repos.contains_key("vendor"));
}

// ============================================================================
// 5. Clone creates files as host UID (not root)
// ============================================================================

#[tokio::test]
#[ignore]
async fn clone_creates_files_as_host_uid() {
    let session = TestSession::new("sync-uid").await;
    // TestRepo uses /var/folders temp which Colima can't see.
    // Create repo under ~/.cache instead.
    let cache_dir = dirs::home_dir().unwrap().join(".cache/gitvm/test-repos");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let repo_path = cache_dir.join("uid-check");
    let _ = std::fs::remove_dir_all(&repo_path);
    let git_repo = git2::Repository::init(&repo_path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    std::fs::write(repo_path.join("README.md"), "# test\n").unwrap();
    let mut index = git_repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree = git_repo.find_tree(index.write_tree().unwrap()).unwrap();
    git_repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[]).unwrap();
    // Use a wrapper that holds the path for cleanup
    struct CleanupPath(std::path::PathBuf);
    impl Drop for CleanupPath { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    let _cleanup = CleanupPath(repo_path.clone());

    // Clone into volume using the sync engine
    let d = docker();
    let engine = gitvm::sync::SyncEngine::new(d.clone());
    let session_name = gitvm::types::SessionName::new(&session.name);

    let clone_result = engine.clone_into_volume(&session_name, "uid-repo", &repo_path, None).await;
    if let Err(ref e) = clone_result {
        eprintln!("Clone error: {:?}", e);
    }
    clone_result.expect("clone should succeed");

    // Check ownership inside the volume
    let check = session.run_simple(
        BASE_IMAGE,
        "stat -c '%u' /workspace/uid-repo/README.md",
    ).await;
    check.assert_success();

    let uid_str = check.stdout.trim();
    let host_uid = format!("{}", unsafe { libc::getuid() });
    assert_eq!(uid_str, host_uid,
        "Cloned file should be owned by host UID {}. Got UID {}", host_uid, uid_str);
}

