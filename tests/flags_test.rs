//! GS-5: Dead flags & unused code cleanup — tests
//!
//! Tests that CLI flags are properly wired through to their effects:
//! - --continue → CONTINUE_SESSION=1 env var
//! - --prompt → CLAUDE_INITIAL_PROMPT env var (base64 encoded)
//! - --squash false → merge commit with two parents
//! - --squash true → squash commit with single parent
//! - role=Dependency → filtered out of sync by default (no extract)

mod harness;

use std::collections::BTreeMap;
use std::path::Path;

// ============================================================================
// build_create_args: --continue flag
// ============================================================================

#[test]
fn continue_flag_sets_env_var() {
    // build_create_args is private, so we test through the public container API.
    // The LaunchOptions struct carries continue/prompt through to build_create_args.
    // We verify the env var by checking the args built with continue_session=true.
    use gitvm::container::LaunchOptions;

    let opts = LaunchOptions {
        continue_session: true,
        initial_prompt: None,
    };

    let env = opts.env_vars();
    assert!(
        env.iter().any(|e| e == "CONTINUE_SESSION=1"),
        "Expected CONTINUE_SESSION=1 in env vars, got: {:?}",
        env
    );
}

#[test]
fn continue_flag_absent_when_false() {
    use gitvm::container::LaunchOptions;

    let opts = LaunchOptions {
        continue_session: false,
        initial_prompt: None,
    };

    let env = opts.env_vars();
    assert!(
        !env.iter().any(|e| e.starts_with("CONTINUE_SESSION")),
        "CONTINUE_SESSION should not be set when false, got: {:?}",
        env
    );
}

// ============================================================================
// build_create_args: --prompt flag
// ============================================================================

#[test]
fn prompt_flag_sets_env_var_base64() {
    use gitvm::container::LaunchOptions;

    let opts = LaunchOptions {
        continue_session: false,
        initial_prompt: Some("Hello Claude".to_string()),
    };

    let env = opts.env_vars();
    let prompt_var = env.iter().find(|e| e.starts_with("CLAUDE_INITIAL_PROMPT="));
    assert!(
        prompt_var.is_some(),
        "Expected CLAUDE_INITIAL_PROMPT in env vars, got: {:?}",
        env
    );

    // The value should be base64-encoded
    let value = prompt_var.unwrap().strip_prefix("CLAUDE_INITIAL_PROMPT=").unwrap();
    assert!(!value.is_empty(), "Prompt value should not be empty");
    // Decoding should give back the original prompt
    let decoded = String::from_utf8(
        base64_decode(value)
    ).expect("valid utf8");
    assert_eq!(decoded.trim(), "Hello Claude");
}

#[test]
fn prompt_flag_absent_when_none() {
    use gitvm::container::LaunchOptions;

    let opts = LaunchOptions {
        continue_session: false,
        initial_prompt: None,
    };

    let env = opts.env_vars();
    assert!(
        !env.iter().any(|e| e.starts_with("CLAUDE_INITIAL_PROMPT")),
        "CLAUDE_INITIAL_PROMPT should not be set when None, got: {:?}",
        env
    );
}

// ============================================================================
// Squash merge: --squash true vs false
// ============================================================================

#[test]
fn squash_true_uses_single_parent() {
    let repo = harness::TestRepo::new("squash-true");
    let main_branch = repo.branch(); // capture before switching

    // Create a session branch with a commit
    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let head = git_repo.head().unwrap().peel_to_commit().unwrap();

    // Create session branch from HEAD
    git_repo.branch("test-session", &head, false).unwrap();

    // Add a commit on the session branch
    git_repo.set_head("refs/heads/test-session").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    std::fs::write(repo.path.join("session-work.txt"), "session work").unwrap();
    let mut index = git_repo.index().unwrap();
    index.add_path(Path::new("session-work.txt")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_id).unwrap();
    let parent = git_repo.head().unwrap().peel_to_commit().unwrap();
    git_repo.commit(Some("HEAD"), &sig, &sig, "session work", &tree, &[&parent]).unwrap();

    // Switch back to main
    git_repo.set_head(&format!("refs/heads/{}", main_branch)).unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    // Merge with squash=true
    let engine = create_sync_engine();
    let result = engine.merge(&repo.path, "test-session", &main_branch, true);
    assert!(result.is_ok(), "Squash merge should succeed: {:?}", result);

    // Verify: the resulting commit should have exactly ONE parent (squash)
    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let merge_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(
        merge_commit.parent_count(), 1,
        "Squash merge commit should have exactly 1 parent, got {}",
        merge_commit.parent_count()
    );
}

#[test]
fn squash_false_uses_merge_commit() {
    let repo = harness::TestRepo::new("squash-false");
    let main_branch = repo.branch(); // capture before switching

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let head = git_repo.head().unwrap().peel_to_commit().unwrap();

    // Create session branch from HEAD
    git_repo.branch("test-session", &head, false).unwrap();

    // Add a commit on the session branch
    git_repo.set_head("refs/heads/test-session").unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    std::fs::write(repo.path.join("session-work.txt"), "session work").unwrap();
    let mut index = git_repo.index().unwrap();
    index.add_path(Path::new("session-work.txt")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_id).unwrap();
    let parent = git_repo.head().unwrap().peel_to_commit().unwrap();
    git_repo.commit(Some("HEAD"), &sig, &sig, "session work", &tree, &[&parent]).unwrap();

    // Also add a commit on main so it's not a fast-forward
    git_repo.set_head(&format!("refs/heads/{}", main_branch)).unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    std::fs::write(repo.path.join("main-work.txt"), "main work").unwrap();
    let mut index = git_repo.index().unwrap();
    index.add_path(Path::new("main-work.txt")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = git_repo.find_tree(tree_id).unwrap();
    let parent = git_repo.head().unwrap().peel_to_commit().unwrap();
    git_repo.commit(Some("HEAD"), &sig, &sig, "main work", &tree, &[&parent]).unwrap();

    // Merge with squash=false — should create a merge commit with two parents
    let engine = create_sync_engine();
    let result = engine.merge(&repo.path, "test-session", &main_branch, false);
    assert!(result.is_ok(), "Regular merge should succeed: {:?}", result);

    // Verify: the resulting commit should have TWO parents (merge commit)
    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let merge_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
    assert_eq!(
        merge_commit.parent_count(), 2,
        "Merge commit should have exactly 2 parents, got {}",
        merge_commit.parent_count()
    );
}

// ============================================================================
// Role: dependency implies no extract in default sync
// ============================================================================

#[test]
fn role_dependency_implies_no_extract() {
    use gitvm::types::config::{SessionConfig, ProjectConfig, RepoRole};

    let mut projects = BTreeMap::new();
    projects.insert("my-project".to_string(), ProjectConfig {
        path: "/tmp/my-project".into(),
        main: false,
        role: RepoRole::Project,
    });
    projects.insert("my-dep".to_string(), ProjectConfig {
        path: "/tmp/my-dep".into(),
        main: false,
        role: RepoRole::Dependency,
    });

    let config = SessionConfig {
        version: Some("1".to_string()),
        projects,
    };

    // project_repos() should only include Project role
    let project_repos = config.project_repos();
    assert!(project_repos.contains_key("my-project"));
    assert!(!project_repos.contains_key("my-dep"));

    // dependency_repos() should only include Dependency role
    let dep_repos = config.dependency_repos();
    assert!(!dep_repos.contains_key("my-project"));
    assert!(dep_repos.contains_key("my-dep"));
}

// ============================================================================
// Helpers
// ============================================================================

fn create_sync_engine() -> gitvm::sync::SyncEngine {
    let docker = harness::docker();
    gitvm::sync::SyncEngine::new(docker)
}

/// Simple base64 decode for test verification
fn base64_decode(input: &str) -> Vec<u8> {
    let output = std::process::Command::new("base64")
        .arg("--decode")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes()).unwrap();
            child.wait_with_output()
        })
        .expect("base64 decode");
    output.stdout
}
