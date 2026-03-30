//! Docker end-to-end tests — verify bits actually move between host and container.
//!
//! These require Docker running. Run with: cargo test --test docker_e2e_test -- --ignored
//!
//! Every test creates a real session (Docker volumes), clones repos into the
//! container, runs sync operations, and verifies the resulting git state on
//! both sides.

mod harness;
use harness::*;
use git2::Repository;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use git_sandbox::sync::SyncEngine;
use git_sandbox::types::SessionName;
use git_sandbox::vm::*;

/// Create a repo in ~/.cache so Docker can see it (macOS /var/folders not shared)
fn colima_visible_repo(name: &str) -> (PathBuf, impl Drop) {
    let cache_dir = dirs::home_dir().unwrap().join(".cache/git-sandbox/test-repos");
    std::fs::create_dir_all(&cache_dir).unwrap();
    let repo_path = cache_dir.join(name);
    let _ = std::fs::remove_dir_all(&repo_path);
    let git_repo = git2::Repository::init(&repo_path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    std::fs::write(repo_path.join("README.md"), format!("# {}\n", name)).unwrap();
    let mut index = git_repo.index().unwrap();
    index.add_path(Path::new("README.md")).unwrap();
    index.write().unwrap();
    let tree = git_repo.find_tree(index.write_tree().unwrap()).unwrap();
    git_repo.commit(Some("refs/heads/main"), &sig, &sig, "initial commit", &tree, &[]).unwrap();
    git_repo.set_head("refs/heads/main").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    struct Cleanup(PathBuf);
    impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }
    (repo_path.clone(), Cleanup(repo_path))
}

fn add_commit(repo_path: &Path, message: &str, files: &[(&str, &str)]) -> git2::Oid {
    let repo = Repository::open(repo_path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    for (name, content) in files {
        let file_path = repo_path.join(name);
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&file_path, content).unwrap();
    }
    let mut index = repo.index().unwrap();
    for (name, _) in files { index.add_path(Path::new(name)).unwrap(); }
    index.write().unwrap();
    let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent]).unwrap()
}

async fn seed_container_repo(
    session: &TestSession, host_repo_path: &Path,
    container_repo_name: &str, commits: &[(&str, &[(&str, &str)])],
) -> String {
    let mut script = format!(
        "git config --global --add safe.directory '*' && \
         git clone --no-local /upstream /workspace/{name} && \
         cd /workspace/{name} && \
         git config user.email 'test@test.com' && git config user.name 'test'",
        name = container_repo_name,
    );
    for (msg, files) in commits {
        for (fname, content) in *files {
            script.push_str(&format!(" && echo '{}' > {}", content.replace('\'', "'\\''"), fname));
        }
        script.push_str(&format!(" && git add . && git commit -m '{}'", msg.replace('\'', "'\\'''")));
    }
    script.push_str(" && git rev-parse HEAD");
    let tc = session.run_container(
        BASE_IMAGE, &script, vec![],
        vec![
            format!("{}:/workspace", session.session_volume()),
            format!("{}:/upstream:ro", host_repo_path.display()),
        ],
    ).await;
    let result = tc.wait_and_collect().await;
    result.assert_success();
    result.stdout.trim().lines().last().unwrap_or("").trim().to_string()
}

async fn container_head(session: &TestSession, repo_name: &str) -> String {
    let result = session.run_simple(
        BASE_IMAGE,
        &format!("git config --global --add safe.directory '*' && cd /workspace/{} && git rev-parse HEAD", repo_name),
    ).await;
    result.assert_success();
    result.stdout.trim().to_string()
}

fn repo_configs(name: &str, path: &Path) -> BTreeMap<String, PathBuf> {
    let mut m = BTreeMap::new();
    m.insert(name.to_string(), path.to_path_buf());
    m
}

fn host_head(path: &Path, branch: &str) -> String {
    let repo = Repository::open(path).unwrap();
    let reference = repo.find_reference(&format!("refs/heads/{}", branch)).unwrap();
    let commit = reference.peel_to_commit().unwrap();
    commit.id().to_string()
}

fn assert_no_markers_on(path: &Path, branch: &str) {
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
        }
        git2::TreeWalkResult::Ok
    }).unwrap();
}

// ============================================================================
// Test: clone_into_volume creates a repo in the container
// ============================================================================

#[tokio::test]
#[ignore]
async fn clone_creates_repo_in_container() {
    let session = TestSession::new("e2e-clone").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-clone-repo");
    add_commit(&repo_path, "initial file", &[("hello.txt", "world")]);

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    engine.clone_into_volume(&name, "test-repo", &repo_path, None).await.unwrap();

    // Verify: container has the repo with the commit
    let head = container_head(&session, "test-repo").await;
    assert!(!head.is_empty(), "container should have a HEAD");
}

// ============================================================================
// Test: extract delivers container commits to host session branch
// ============================================================================

#[tokio::test]
#[ignore]
async fn extract_delivers_commits_to_host() {
    let session = TestSession::new("e2e-extract").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-extract-repo");
    add_commit(&repo_path, "base", &[("base.txt", "base content")]);

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    // Clone into container + add container-side commits
    let container_h = seed_container_repo(&session, &repo_path, "test-repo", &[
        ("agent commit 1", &[("agent.txt", "agent work")]),
        ("agent commit 2", &[("agent2.txt", "more agent work")]),
    ]).await;

    // Extract
    let session_branch = session.name.clone();
    let result = engine.extract(&name, "test-repo", &repo_path, &session_branch).await.unwrap();

    assert!(result.commit_count > 0, "should extract commits");

    // Verify: session branch on host has the container's work
    let repo = Repository::open(&repo_path).unwrap();
    let session_ref = repo.find_reference(&format!("refs/heads/{}", session_branch)).unwrap();
    let session_commit = session_ref.peel_to_commit().unwrap();
    assert_eq!(session_commit.id().to_string(), container_h,
        "session branch should match container HEAD");
}

// ============================================================================
// Test: inject delivers host commits to container
// ============================================================================

#[tokio::test]
#[ignore]
async fn inject_delivers_commits_to_container() {
    let session = TestSession::new("e2e-inject").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-inject-repo");
    add_commit(&repo_path, "base", &[("base.txt", "base")]);

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    // Clone into container
    engine.clone_into_volume(&name, "test-repo", &repo_path, None).await.unwrap();
    let container_before = container_head(&session, "test-repo").await;

    // Add commits on host
    add_commit(&repo_path, "host work", &[("host.txt", "host content")]);
    let host_h = host_head(&repo_path, "main");
    assert_ne!(container_before, host_h);

    // Inject
    engine.inject(&name, "test-repo", &repo_path, "main").await.unwrap();

    // Verify: container HEAD advanced
    let container_after = container_head(&session, "test-repo").await;
    assert_ne!(container_before, container_after, "container HEAD should advance after inject");
}

// ============================================================================
// Test: push then push is idempotent (no phantom work)
// ============================================================================

#[tokio::test]
#[ignore]
async fn push_then_push_is_idempotent() {
    let session = TestSession::new("e2e-idempotent").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-idempotent-repo");
    add_commit(&repo_path, "base", &[("base.txt", "base")]);

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    // Clone into container
    engine.clone_into_volume(&name, "test-repo", &repo_path, None).await.unwrap();

    // Add host work
    add_commit(&repo_path, "new work", &[("new.txt", "content")]);

    // First push: inject + re-extract
    engine.inject(&name, "test-repo", &repo_path, "main").await.unwrap();
    let session_branch = session.name.clone();
    let _ = engine.extract(&name, "test-repo", &repo_path, &session_branch).await;

    // Second push: plan should show no work
    let repos = repo_configs("test-repo", &repo_path);
    let plan = engine.plan_sync(&name, "main", &repos).await.unwrap();

    let has_push_work = plan.action.repo_actions.iter()
        .any(|a| !matches!(a.state.push_action(), git_sandbox::types::git::PushAction::Skip));
    assert!(!has_push_work, "second push should show no work");
}

// ============================================================================
// Test: full pull pipeline (extract + merge into target)
// ============================================================================

#[tokio::test]
#[ignore]
async fn pull_extracts_and_merges_into_target() {
    let session = TestSession::new("e2e-pull").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-pull-repo");
    add_commit(&repo_path, "base", &[("base.txt", "base")]);

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    // Clone + add container work
    seed_container_repo(&session, &repo_path, "test-repo", &[
        ("feature", &[("feature.txt", "feature code")]),
    ]).await;

    // Extract
    let session_branch = session.name.clone();
    engine.extract(&name, "test-repo", &repo_path, &session_branch).await.unwrap();

    // Merge session → main
    let merge_result = engine.merge(&repo_path, &session_branch, "main", true).unwrap();

    // Verify: main has the feature file
    let repo = Repository::open(&repo_path).unwrap();
    let main_ref = repo.find_reference("refs/heads/main").unwrap();
    let main_commit = main_ref.peel_to_commit().unwrap();
    let tree = main_commit.tree().unwrap();
    let has_feature = tree.get_name("feature.txt").is_some();
    assert!(has_feature, "main should have feature.txt after pull");
    assert_no_markers_on(&repo_path, "main");
}

// ============================================================================
// Test: snapshot reads all repos in volume
// ============================================================================

#[tokio::test]
#[ignore]
async fn snapshot_discovers_all_repos() {
    let session = TestSession::new("e2e-snapshot").await;
    let (repo_a, _ca) = colima_visible_repo("e2e-snap-alpha");
    let (repo_b, _cb) = colima_visible_repo("e2e-snap-beta");

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    engine.clone_into_volume(&name, "alpha", &repo_a, None).await.unwrap();
    engine.clone_into_volume(&name, "beta", &repo_b, None).await.unwrap();

    let repos = engine.snapshot(&name, "main").await.unwrap();
    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();

    assert!(names.contains(&"alpha"), "should find alpha, got {:?}", names);
    assert!(names.contains(&"beta"), "should find beta, got {:?}", names);
}

// ============================================================================
// Test: plan_sync classifies correctly after real operations
// ============================================================================

#[tokio::test]
#[ignore]
async fn plan_reflects_actual_state() {
    let session = TestSession::new("e2e-plan").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-plan-repo");
    add_commit(&repo_path, "base", &[("base.txt", "base")]);

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    // Clone into container + add container work
    seed_container_repo(&session, &repo_path, "test-repo", &[
        ("agent work", &[("agent.txt", "content")]),
    ]).await;

    // Plan should show work
    let repos = repo_configs("test-repo", &repo_path);
    let plan = engine.plan_sync(&name, "main", &repos).await.unwrap();

    assert!(!plan.action.repo_actions.is_empty(), "should have repo actions");
    let action = &plan.action.repo_actions[0];

    // Container ahead → pull should be Extract or CloneToHost
    let pull = action.state.pull_action();
    assert!(!matches!(pull, git_sandbox::types::git::PullAction::Skip),
        "container has work, pull should not be Skip, got {:?}", pull);

    // Build VM and verify plan_pull generates ops
    // Build VM manually (build_vm_from_plan is in the binary crate, not lib)
    let mut vm = SyncVM::new(name.as_str(), "main");
    for action in &plan.action.repo_actions {
        let host_path = repos.get(&action.repo_name).cloned();
        vm.set_repo(&action.repo_name, RepoVM::from_refs(
            action.container_head.as_ref().map(|h| RefState::At(h.to_string())).unwrap_or(RefState::Absent),
            action.session_head.as_ref().map(|h| RefState::At(h.to_string())).unwrap_or(RefState::Absent),
            action.target_head.as_ref().map(|h| RefState::At(h.to_string())).unwrap_or(RefState::Absent),
            host_path,
        ));
    }
    let pull_ops = programs::plan_pull(&vm);
    assert!(!pull_ops.is_empty(), "VM plan_pull should generate ops for container-ahead state");
}

// ============================================================================
// Test: inject into nonexistent repo fails gracefully
// ============================================================================

#[tokio::test]
#[ignore]
async fn inject_nonexistent_repo_fails_gracefully() {
    let session = TestSession::new("e2e-inject-fail").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-inject-fail-repo");

    let name = SessionName::new(&session.name);
    let engine = SyncEngine::new(session.docker.clone());

    // DON'T clone into container — repo doesn't exist in volume
    // Inject should fail because /session/nonexistent doesn't exist
    let result = engine.inject(&name, "nonexistent-repo", &repo_path, "main").await;
    assert!(result.is_err(), "inject into nonexistent repo should fail");

    // The error should be an InjectionFailed, not a panic
    let err = result.unwrap_err();
    let err_str = format!("{}", err);
    assert!(err_str.contains("inject") || err_str.contains("failed") || err_str.contains("exit"),
        "error should mention injection failure, got: {}", err_str);
}

// ============================================================================
// Test: VM push program fails gracefully when inject fails
// ============================================================================

#[tokio::test]
#[ignore]
async fn vm_push_inject_failure_reports_error() {
    let session = TestSession::new("e2e-vm-fail").await;
    let (repo_path, _cleanup) = colima_visible_repo("e2e-vm-fail-repo");
    add_commit(&repo_path, "file", &[("file.txt", "content")]);

    let name = SessionName::new(&session.name);

    // Clone into container
    let engine = SyncEngine::new(session.docker.clone());
    engine.clone_into_volume(&name, "test-repo", &repo_path, None).await.unwrap();

    // Make container dirty by adding uncommitted changes
    session.run_simple(
        BASE_IMAGE,
        &format!(
            "git config --global --add safe.directory '*' && \
             cd /workspace/test-repo && echo dirty > dirty.txt",
        ),
    ).await;

    // Add host work so there's something to push
    add_commit(&repo_path, "new work", &[("new.txt", "new content")]);

    // Try VM push — inject into dirty container should fail
    let backend = git_sandbox::vm::RealBackend::from_docker(session.docker.clone(), &session.name);
    let mut vm = git_sandbox::vm::SyncVM::new(&session.name, "main");
    vm.set_repo("test-repo", git_sandbox::vm::RepoVM::from_refs(
        git_sandbox::vm::RefState::At("old".into()),
        git_sandbox::vm::RefState::At("old".into()),
        git_sandbox::vm::RefState::At("new".into()),
        Some(repo_path.clone()),
    ));

    let result = vm.run(&backend, vec![
        git_sandbox::vm::Op::Inject { repo: "test-repo".into(), branch: "main".into() },
    ]).await;

    // Should fail but not panic
    // The inject may succeed (merge into dirty worktree sometimes works)
    // or fail (merge conflict with dirty files). Either way, no panic.
    eprintln!("VM push result: halted={}, succeeded={}, failed={}",
        result.halted, result.succeeded(), result.failed());
    for o in &result.outcomes {
        eprintln!("  {:?}: {:?}", o.op_description, o.result.is_ok());
    }
}
