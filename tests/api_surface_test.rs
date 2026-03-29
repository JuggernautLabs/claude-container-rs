//! API surface integration tests — exercises the highest-level commands
//! through real Docker containers and git repos.
//!
//! Each test sets up a specific environment to hit a specific execution path,
//! then asserts on properties of the result.
//!
//! Run: cargo test --test api_surface_test -- --ignored --nocapture --test-threads=1

mod harness;
use harness::*;
use git2::Repository;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use git_sandbox::sync::SyncEngine;
use git_sandbox::types::{SessionName, CommitHash};
use git_sandbox::types::git::*;
use git_sandbox::types::action::*;

/// Helper: create a TestRepo under ~/.cache so Colima can see it (macOS /var/folders not shared)
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
    git_repo.commit(Some("HEAD"), &sig, &sig, "initial commit", &tree, &[]).unwrap();

    struct Cleanup(PathBuf);
    impl Drop for Cleanup { fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); } }

    (repo_path.clone(), Cleanup(repo_path))
}

/// Helper: add a commit to a repo at a given path
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
    for (name, _) in files {
        index.add_path(Path::new(name)).unwrap();
    }
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let parent = repo.head().unwrap().peel_to_commit().unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, message, &tree, &[&parent]).unwrap()
}

/// Helper: clone host repo into session volume, add container-side commits
async fn seed_container_repo(
    session: &TestSession,
    host_repo_path: &Path,
    container_repo_name: &str,
    commits: &[(&str, &[(&str, &str)])],  // (message, [(file, content)])
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
            script.push_str(&format!(
                " && echo '{}' > {}",
                content.replace('\'', "'\\''"), fname,
            ));
        }
        script.push_str(&format!(
            " && git add . && git commit -m '{}'",
            msg.replace('\'', "'\\''"),
        ));
    }

    script.push_str(" && git rev-parse HEAD");

    let tc = session.run_container(
        BASE_IMAGE,
        &script,
        vec![],
        vec![
            format!("{}:/workspace", session.session_volume()),
            format!("{}:/upstream:ro", host_repo_path.display()),
        ],
    ).await;
    let result = tc.wait_and_collect().await;
    result.assert_success();
    // Last line is the git rev-parse HEAD output; earlier lines are commit messages
    result.stdout.trim().lines().last().unwrap_or("").trim().to_string()
}

/// Helper: get HEAD of a repo in the session volume
async fn container_head(session: &TestSession, repo_name: &str) -> String {
    let result = session.run_simple(
        BASE_IMAGE,
        &format!(
            "git config --global --add safe.directory '*' && \
             cd /workspace/{} && git rev-parse HEAD",
            repo_name
        ),
    ).await;
    result.assert_success();
    result.stdout.trim().to_string()
}

fn repo_configs(name: &str, path: &Path) -> BTreeMap<String, PathBuf> {
    let mut m = BTreeMap::new();
    m.insert(name.to_string(), path.to_path_buf());
    m
}

/// Get the HEAD commit SHA as a string
fn git_head(path: &Path) -> String {
    let r = Repository::open(path).unwrap();
    let head = r.head().unwrap();
    let commit = head.peel_to_commit().unwrap();
    commit.id().to_string()
}

/// Get the current branch name
fn git_branch(path: &Path) -> String {
    let r = Repository::open(path).unwrap();
    let head = r.head().unwrap();
    head.shorthand().unwrap_or("HEAD").to_string()
}

// ============================================================================
// 1. EXTRACT: container commits land on session branch on host
// ============================================================================

#[tokio::test]
#[ignore]
async fn extract_puts_container_commits_on_session_branch() {
    let session = TestSession::new("api-extract").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-extract-repo");

    // Seed container with 2 commits
    let ctr_head = seed_container_repo(&session, &repo_path, "repo", &[
        ("work 1", &[("a.txt", "aaa")]),
        ("work 2", &[("b.txt", "bbb")]),
    ]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);

    let result = engine.extract(&sn, "repo", &repo_path, &sn.to_string()).await;
    assert!(result.is_ok(), "extract failed: {:?}", result.err());
    let ext = result.unwrap();

    // Property: commit_count > 0
    assert!(ext.commit_count > 0, "should have extracted commits, got {}", ext.commit_count);

    // Property: session branch exists on host
    let host_repo = Repository::open(&repo_path).unwrap();
    let branch = host_repo.find_branch(&sn.to_string(), git2::BranchType::Local);
    assert!(branch.is_ok(), "session branch should exist on host");

    // Property: session branch HEAD == container HEAD
    let branch_head = branch.unwrap().get().peel_to_commit().unwrap().id().to_string();
    assert_eq!(branch_head, ctr_head, "session branch should match container HEAD");

    // Property: the container's files are reachable from the session branch
    let commit = host_repo.find_commit(git2::Oid::from_str(&ctr_head).unwrap()).unwrap();
    let tree = commit.tree().unwrap();
    assert!(tree.get_name("a.txt").is_some(), "a.txt should be in tree");
    assert!(tree.get_name("b.txt").is_some(), "b.txt should be in tree");
}

// ============================================================================
// 2. EXTRACT then MERGE: full pull pipeline squash-merges to target
// ============================================================================

#[tokio::test]
#[ignore]
async fn pull_pipeline_squash_merges_to_target() {
    let session = TestSession::new("api-pull").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-pull-repo");
    let host_main_before = git_head(&repo_path);

    // 3 commits in container
    seed_container_repo(&session, &repo_path, "repo", &[
        ("feature part 1", &[("feat.rs", "fn feat() {}")]),
        ("feature part 2", &[("feat.rs", "fn feat() { v2 }")]),
        ("feature part 3", &[("feat_test.rs", "fn test() {}")]),
    ]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let session_branch = sn.to_string();
    let main_name = git_branch(&repo_path);

    // Extract
    let ext = engine.extract(&sn, "repo", &repo_path, &session_branch).await.unwrap();
    assert!(ext.commit_count > 0);

    // Squash merge session → main
    let merge = engine.merge(&repo_path, &session_branch, &main_name, true);
    assert!(merge.is_ok(), "merge failed: {:?}", merge.err());
    let outcome = merge.unwrap();

    // Property: outcome is SquashMerge
    assert!(
        matches!(outcome, MergeOutcome::SquashMerge { .. }),
        "expected SquashMerge, got {:?}", outcome
    );

    // Property: main moved forward
    let host_main_after = git_head(&repo_path);
    assert_ne!(host_main_before, host_main_after, "main should advance");

    // Property: squash commit has single parent (not a merge commit)
    let r = Repository::open(&repo_path).unwrap();
    let head = r.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(head.parent_count(), 1, "squash = 1 parent");

    // Property: container's files landed on main
    assert!(repo_path.join("feat.rs").exists());
    assert!(repo_path.join("feat_test.rs").exists());

    // Property: squash-base ref was set
    let squash_ref = r.find_reference(
        &format!("refs/claude-container/squash-base/{}", session_branch)
    );
    assert!(squash_ref.is_ok(), "squash-base ref should exist");
}

// ============================================================================
// 3. SECOND PULL: only new commits get squashed (squash-base tracking)
// ============================================================================

#[tokio::test]
#[ignore]
async fn second_pull_only_squashes_new_commits() {
    let session = TestSession::new("api-incr").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-incr-repo");

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let session_branch = sn.to_string();
    let main_name = git_branch(&repo_path);

    // Round 1: seed + extract + merge
    seed_container_repo(&session, &repo_path, "repo", &[
        ("round 1", &[("r1.txt", "round1")]),
    ]).await;
    engine.extract(&sn, "repo", &repo_path, &session_branch).await.unwrap();
    let merge1 = engine.merge(&repo_path, &session_branch, &main_name, true).unwrap();
    // Container cloned from host (shared initial commit) then added 1 commit
    assert!(matches!(merge1, MergeOutcome::SquashMerge { commits: 1, .. }),
        "first merge should squash 1 commit (r1), got {:?}", merge1);

    let main_after_r1 = git_head(&repo_path);

    // Round 2: add more work in container, extract, merge
    let tc = session.run_container(
        BASE_IMAGE,
        "git config --global --add safe.directory '*' && \
         cd /workspace/repo && \
         git config user.email 'test@test.com' && git config user.name 'test' && \
         echo round2 > r2.txt && git add . && git commit -m 'round 2' && \
         echo round2b > r2b.txt && git add . && git commit -m 'round 2b' && \
         git rev-parse HEAD",
        vec![],
        vec![format!("{}:/workspace", session.session_volume())],
    ).await;
    let result = tc.wait_and_collect().await;
    result.assert_success();

    engine.extract(&sn, "repo", &repo_path, &session_branch).await.unwrap();
    let merge2 = engine.merge(&repo_path, &session_branch, &main_name, true).unwrap();

    // Property: second merge only squashes the 2 NEW commits, not all 4
    assert!(
        matches!(merge2, MergeOutcome::SquashMerge { commits: 2, .. }),
        "second merge should squash exactly 2 new commits, got {:?}", merge2
    );

    // Property: main advanced again
    let main_after_r2 = git_head(&repo_path);
    assert_ne!(main_after_r1, main_after_r2);
}

// ============================================================================
// 4. PLAN_SYNC: classifies repos correctly based on state
// ============================================================================

#[tokio::test]
#[ignore]
async fn plan_sync_classifies_new_container_work() {
    let session = TestSession::new("api-plan").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-plan-repo");

    seed_container_repo(&session, &repo_path, "repo", &[
        ("new work", &[("new.txt", "content")]),
    ]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let session_branch = sn.to_string();
    let configs = repo_configs("repo", &repo_path);
    let main_name = git_branch(&repo_path);

    // First plan (no session branch) → CloneToHost
    let plan = engine.plan_sync(&sn, &main_name, &configs).await.unwrap();
    assert!(plan.destructive, "plan should be destructive");
    assert_eq!(plan.action.repo_actions.len(), 1);
    let action = &plan.action.repo_actions[0];
    assert!(
        matches!(action.state.pull_action(), PullAction::CloneToHost),
        "first sync should be CloneToHost, got {:?}", action.state.pull_action()
    );

    // Extract to create session branch
    engine.extract(&sn, "repo", &repo_path, &session_branch).await.unwrap();

    // Second plan (session branch exists, container ahead of target) → MergeToTarget
    let plan2 = engine.plan_sync(&sn, &main_name, &configs).await.unwrap();
    let action2 = &plan2.action.repo_actions[0];
    assert!(
        matches!(action2.state.pull_action(), PullAction::MergeToTarget { .. }),
        "after extract, should be MergeToTarget, got {:?}", action2.state.pull_action()
    );
}

// ============================================================================
// 5. PLAN_SYNC: identical state → Skip
// ============================================================================

#[tokio::test]
#[ignore]
async fn plan_sync_skips_when_already_synced() {
    let session = TestSession::new("api-skip").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-skip-repo");

    // Clone into volume — no extra commits
    seed_container_repo(&session, &repo_path, "repo", &[]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let session_branch = sn.to_string();
    let configs = repo_configs("repo", &repo_path);
    let main_name = git_branch(&repo_path);

    // First sync: extract to create session branch (initial state)
    engine.extract(&sn, "repo", &repo_path, &session_branch).await.unwrap();

    // Now plan again — session branch == container HEAD, both identical to host
    let plan = engine.plan_sync(&sn, &main_name, &configs).await.unwrap();

    // Property: plan is NOT destructive (already synced)
    assert!(!plan.destructive, "no-op plan should not be destructive");

    // Property: no work to do
    let action = &plan.action.repo_actions[0];
    assert!(
        !action.state.has_work(),
        "identical state should have no work, got pull={:?} push={:?}",
        action.state.pull_action(), action.state.push_action()
    );
}

// ============================================================================
// 6. EXECUTE_SYNC: full round-trip Pull → SyncResult
// ============================================================================

#[tokio::test]
#[ignore]
async fn execute_sync_pull_succeeds() {
    let session = TestSession::new("api-exec").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-exec-repo");

    seed_container_repo(&session, &repo_path, "repo", &[
        ("executed work", &[("exec.txt", "done")]),
    ]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let configs = repo_configs("repo", &repo_path);
    let main_name = git_branch(&repo_path);

    let plan = engine.plan_sync(&sn, &main_name, &configs).await.unwrap();
    let sync_result = engine.execute_sync(&sn, plan.action, &configs).await;
    assert!(sync_result.is_ok(), "execute_sync failed: {:?}", sync_result.err());
    let result = sync_result.unwrap();

    // Property: 1 succeeded, 0 failed
    assert_eq!(result.succeeded(), 1, "1 repo should succeed");
    assert_eq!(result.failed(), 0, "0 repos should fail");
    assert_eq!(result.skipped(), 0);

    // Property: first sync for a repo without session branch → ClonedToHost
    // (Pull happens when session branch already exists; CloneToHost is the initial case)
    match &result.results[0] {
        RepoSyncResult::ClonedToHost { extract, .. } => {
            assert!(extract.commit_count > 0, "should extract commits");
        }
        RepoSyncResult::Pulled { extract, merge, .. } => {
            assert!(extract.commit_count > 0, "should extract commits");
            assert!(matches!(merge, MergeOutcome::SquashMerge { .. }),
                "should squash-merge, got {:?}", merge);
        }
        other => panic!("expected ClonedToHost or Pulled, got {:?}", other),
    }
}

// ============================================================================
// 7. EXECUTE_SYNC with MergeToTarget: no extraction, uses Merged variant
// ============================================================================

#[tokio::test]
#[ignore]
async fn execute_sync_merge_to_target_no_extraction() {
    let session = TestSession::new("api-mt").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-mt-repo");

    seed_container_repo(&session, &repo_path, "repo", &[
        ("mt work", &[("mt.txt", "merged")]),
    ]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let session_branch = sn.to_string();
    let main_name = git_branch(&repo_path);

    // Step 1: extract (creates session branch)
    engine.extract(&sn, "repo", &repo_path, &session_branch).await.unwrap();

    // Step 2: now plan_sync — container HEAD == session HEAD, but session ahead of main
    // This should produce MergeToTarget
    let plan = engine.plan_sync(&sn, &main_name, &repo_configs("repo", &repo_path)).await.unwrap();

    let action = &plan.action.repo_actions[0];
    assert!(
        matches!(action.state.pull_action(), PullAction::MergeToTarget { .. }),
        "should be MergeToTarget after extract without merge, got {:?}", action.state.pull_action()
    );

    // Step 3: execute — should produce Merged (not Pulled with 0 commits)
    let result = engine.execute_sync(
        &sn, plan.action, &repo_configs("repo", &repo_path)
    ).await.unwrap();

    assert_eq!(result.succeeded(), 1);
    match &result.results[0] {
        RepoSyncResult::Merged { merge, .. } => {
            assert!(matches!(merge, MergeOutcome::SquashMerge { .. }),
                "should squash-merge, got {:?}", merge);
        }
        other => panic!("expected Merged variant (no extraction), got {:?}", other),
    }
}

// ============================================================================
// 8. CONFLICT DETECTION: diverged repos surface conflict files
// ============================================================================

#[tokio::test]
#[ignore]
async fn pull_conflict_surfaces_file_list() {
    let session = TestSession::new("api-conflict").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-conflict-repo");

    // Container edits README.md
    seed_container_repo(&session, &repo_path, "repo", &[
        ("container edit", &[("README.md", "container version\n")]),
    ]).await;

    // Host also edits README.md (diverge!)
    add_commit(&repo_path, "host edit", &[("README.md", "host version\n")]);

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let session_branch = sn.to_string();
    let main_name = git_branch(&repo_path);

    // Extract container work to session branch
    engine.extract(&sn, "repo", &repo_path, &session_branch).await.unwrap();

    // Merge should detect conflict
    let merge = engine.merge(&repo_path, &session_branch, &main_name, true).unwrap();

    // Property: merge outcome is Conflict
    assert!(matches!(&merge, MergeOutcome::Conflict { .. }),
        "expected Conflict, got {:?}", merge);

    // Property: conflict files include README.md
    if let MergeOutcome::Conflict { files } = &merge {
        assert!(files.contains(&"README.md".to_string()),
            "README.md should be in conflict list, got {:?}", files);
    }
}

// ============================================================================
// 9. SNAPSHOT: reads all repos from volume
// ============================================================================

#[tokio::test]
#[ignore]
async fn snapshot_reads_all_volume_repos() {
    let session = TestSession::new("api-snap").await;
    let (repo_path_a, _cleanup_a) = colima_visible_repo("api-snap-a");
    let (repo_path_b, _cleanup_b) = colima_visible_repo("api-snap-b");

    // Put two repos in volume
    seed_container_repo(&session, &repo_path_a, "alpha", &[
        ("alpha work", &[("alpha.txt", "a")]),
    ]).await;
    seed_container_repo(&session, &repo_path_b, "beta", &[
        ("beta work", &[("beta.txt", "b")]),
    ]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);

    let repos = engine.snapshot(&sn, "main").await;
    assert!(repos.is_ok(), "snapshot failed: {:?}", repos.err());
    let repos = repos.unwrap();

    // Property: found both repos
    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    assert!(names.contains(&"alpha"), "should find alpha, got {:?}", names);
    assert!(names.contains(&"beta"), "should find beta, got {:?}", names);

    // Property: each repo has a valid HEAD
    for repo in &repos {
        assert!(!repo.head.as_str().is_empty(), "repo {} should have a HEAD", repo.name);
        assert!(!repo.head.as_str().contains('?'), "repo {} HEAD should be valid", repo.name);
    }
}

// ============================================================================
// 10. CLASSIFY: before extraction → host Missing, after extraction → ContainerAhead
// ============================================================================

#[tokio::test]
#[ignore]
async fn classify_before_and_after_extraction() {
    let session = TestSession::new("api-class").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-class-repo");

    let _ctr_head = seed_container_repo(&session, &repo_path, "repo", &[
        ("new work", &[("new.txt", "x")]),
    ]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);

    let repos = engine.snapshot(&sn, "master").await.unwrap();
    let vr = repos.iter().find(|r| r.name == "repo").unwrap();

    // BEFORE extraction: no session branch on host → host is Missing, relation None
    let pair = engine.classify_repo("repo", vr, &repo_path, &sn.to_string(), "master");
    assert!(matches!(pair.container, GitSide::Clean { .. }),
        "container should be Clean, got {:?}", pair.container);
    assert!(matches!(pair.host, GitSide::Missing),
        "host should be Missing before extraction, got {:?}", pair.host);
    assert!(pair.relation.is_none(),
        "no relation possible before extraction");

    // AFTER extraction: session branch exists, relation is ContainerAhead
    engine.extract(&sn, "repo", &repo_path, &sn.to_string()).await.unwrap();

    let pair = engine.classify_repo("repo", vr, &repo_path, &sn.to_string(), "master");
    assert!(pair.relation.is_some(), "should have relation after extraction");
    let rel = pair.relation.unwrap();
    assert!(
        matches!(rel.ancestry, Ancestry::Same),
        "after extraction, session branch == container HEAD → Same, got {:?}", rel.ancestry
    );
}

// ============================================================================
// 11. INJECT: host branch appears in container
// ============================================================================

#[tokio::test]
#[ignore]
async fn inject_pushes_host_branch_to_container() {
    let session = TestSession::new("api-inject").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-inject-repo");

    // Use clone_into_volume (handles ownership correctly)
    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    engine.clone_into_volume(&sn, "repo", &repo_path, None).await.unwrap();

    // Add work on host
    add_commit(&repo_path, "host work", &[("host.txt", "from host")]);
    let host_head = git_head(&repo_path);
    let main_name = git_branch(&repo_path);

    let result = engine.inject(&sn, "repo", &repo_path, &main_name).await;
    assert!(result.is_ok(), "inject failed: {:?}", result.err());

    // Property: container repo now has the host's HEAD
    let ctr_head = container_head(&session, "repo").await;
    assert_eq!(ctr_head, host_head, "container should have host HEAD after inject");
}

// ============================================================================
// 12. CLONE_INTO_VOLUME: files owned by host UID, not root
// ============================================================================

#[tokio::test]
#[ignore]
async fn clone_into_volume_sets_correct_ownership() {
    let session = TestSession::new("api-own").await;
    let (repo_path, _cleanup) = colima_visible_repo("api-own-repo");

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);

    engine.clone_into_volume(&sn, "repo", &repo_path, None).await.unwrap();

    // Property: files owned by host UID
    let check = session.run_simple(
        BASE_IMAGE,
        "stat -c '%u' /workspace/repo/README.md",
    ).await;
    check.assert_success();
    let file_uid = check.stdout.trim();
    let host_uid = format!("{}", unsafe { libc::getuid() });
    assert_eq!(file_uid, host_uid,
        "cloned file should be owned by host UID {}, got {}", host_uid, file_uid);
}

// ============================================================================
// 13. MULTI-REPO PLAN: multiple repos classified independently
// ============================================================================

#[tokio::test]
#[ignore]
async fn plan_sync_handles_multiple_repos() {
    let session = TestSession::new("api-multi").await;
    let (repo_a, _ca) = colima_visible_repo("api-multi-a");
    let (repo_b, _cb) = colima_visible_repo("api-multi-b");

    // Repo A: container has new work (→ Pull)
    seed_container_repo(&session, &repo_a, "alpha", &[
        ("alpha work", &[("a.txt", "a")]),
    ]).await;

    // Repo B: container == host (→ Skip)
    seed_container_repo(&session, &repo_b, "beta", &[]).await;

    let engine = SyncEngine::new(docker());
    let sn = SessionName::new(&session.name);
    let mut configs = BTreeMap::new();
    configs.insert("alpha".to_string(), repo_a.clone());
    configs.insert("beta".to_string(), repo_b.clone());

    let main_name = git_branch(&repo_a);

    let plan = engine.plan_sync(&sn, &main_name, &configs).await.unwrap();

    // Property: 2 actions total
    assert_eq!(plan.action.repo_actions.len(), 2,
        "should have 2 repo actions, got {}", plan.action.repo_actions.len());

    // Property: alpha has new work (CloneToHost on first sync), beta is in sync
    let alpha = plan.action.repo_actions.iter().find(|a| a.repo_name == "alpha").unwrap();
    let beta = plan.action.repo_actions.iter().find(|a| a.repo_name == "beta").unwrap();

    assert!(matches!(alpha.state.pull_action(), PullAction::CloneToHost | PullAction::Extract { .. }),
        "alpha should need sync, got {:?}", alpha.state.pull_action());
    // beta also hasn't been extracted yet, so it's CloneToHost too (even though no new commits)
    assert!(matches!(beta.state.pull_action(), PullAction::CloneToHost | PullAction::Skip),
        "beta should be CloneToHost or Skip, got {:?}", beta.state.pull_action());
}

// ============================================================================
// 14. DIFF: compute_diff returns accurate stats
// ============================================================================

#[test]
fn compute_diff_returns_file_stats() {
    let repo = TestRepo::new("diff-stats");
    let before = repo.head();

    repo.commit("add files", &[
        ("new.txt", "line1\nline2\nline3\n"),
        ("README.md", "# updated\n"),
    ]);
    let after = repo.head();

    let engine = SyncEngine::new(docker());
    let diff = engine.compute_diff(
        &repo.path,
        &CommitHash::new(before),
        &CommitHash::new(after),
    );

    assert!(diff.is_some(), "should return diff");
    let diff = diff.unwrap();

    // Property: correct file count
    assert_eq!(diff.files_changed, 2, "2 files changed, got {}", diff.files_changed);

    // Property: has insertions
    assert!(diff.insertions > 0, "should have insertions");

    // Property: individual files listed
    let paths: Vec<&str> = diff.files.iter().map(|f| f.path.as_str()).collect();
    assert!(paths.contains(&"new.txt"), "new.txt should be in diff, got {:?}", paths);
}

// ============================================================================
// 15. ANCESTRY: check_ancestry reports correct relationship
// ============================================================================

#[test]
fn check_ancestry_reports_ahead_count() {
    let repo = TestRepo::new("ancestry-check");
    let base = repo.head();

    repo.commit("c1", &[("f1.txt", "1")]);
    repo.commit("c2", &[("f2.txt", "2")]);
    repo.commit("c3", &[("f3.txt", "3")]);
    let tip = repo.head();

    let engine = SyncEngine::new(docker());
    let ancestry = engine.check_ancestry(
        &repo.path,
        &CommitHash::new(tip),    // "container" = tip
        &CommitHash::new(base),   // "host" = base
    );

    // Property: tip is ahead of base by 3
    assert!(matches!(ancestry, Ancestry::ContainerAhead { container_ahead: 3 }),
        "expected ContainerAhead {{ container_ahead: 3 }}, got {:?}", ancestry);
}

#[test]
fn check_ancestry_detects_divergence() {
    let repo = TestRepo::new("ancestry-div");

    // Create two diverging branches
    let git_repo = Repository::open(&repo.path).unwrap();
    let head = git_repo.head().unwrap().peel_to_commit().unwrap();

    git_repo.branch("branch-a", &head, false).unwrap();
    git_repo.set_head("refs/heads/branch-a").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    repo.commit("a1", &[("a.txt", "a")]);
    let a_head = repo.head();

    git_repo.set_head("refs/heads/master").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();
    repo.commit("b1", &[("b.txt", "b")]);
    repo.commit("b2", &[("b2.txt", "b2")]);
    let b_head = repo.head();

    let engine = SyncEngine::new(docker());
    let ancestry = engine.check_ancestry(
        &repo.path,
        &CommitHash::new(a_head),
        &CommitHash::new(b_head),
    );

    // Property: diverged with correct counts
    match ancestry {
        Ancestry::Diverged { container_ahead, host_ahead, .. } => {
            assert_eq!(container_ahead, 1, "branch-a has 1 commit");
            assert_eq!(host_ahead, 2, "master has 2 commits");
        }
        other => panic!("expected Diverged, got {:?}", other),
    }
}
