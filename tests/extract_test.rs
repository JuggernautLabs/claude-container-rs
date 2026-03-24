//! Tests for extract accuracy: commit counting, bundle ref cleanup, targeted bundles.
//!
//! These are unit tests that exercise the extract-related logic using local git repos
//! (no Docker required). The Docker-based integration tests are marked #[ignore].

mod harness;

use git2::Repository;
use std::path::Path;

// ============================================================================
// Helper: simulate what extract() does on the host side after receiving a bundle
// ============================================================================

/// Create a bundle from `source_repo_path` and fetch it into `host_repo_path`,
/// updating the session branch. Returns (commit_count, new_head_oid).
///
/// This replicates the host-side logic of SyncEngine::extract() so we can
/// test commit counting and ref cleanup without Docker.
fn simulate_extract_host_side(
    source_repo_path: &Path,
    host_repo_path: &Path,
    session_branch: &str,
) -> (u32, git2::Oid) {
    let source_repo = Repository::open(source_repo_path).unwrap();

    // Create a bundle from HEAD (targeted, not --all)
    let bundle_dir = tempfile::tempdir().unwrap();
    let bundle_path = bundle_dir.path().join("repo.bundle");

    // Determine what to bundle: if HEAD is detached, use the SHA directly
    let head_ref = source_repo.head().unwrap();
    let head_oid = head_ref.peel_to_commit().unwrap().id();

    let bundle_arg = if head_ref.is_branch() {
        // Bundle the branch name
        let branch_name = head_ref.shorthand().unwrap();
        format!("refs/heads/{}", branch_name)
    } else {
        // Detached HEAD: bundle HEAD
        "HEAD".to_string()
    };

    let output = std::process::Command::new("git")
        .args([
            "-C",
            &source_repo_path.to_string_lossy(),
            "bundle",
            "create",
            &bundle_path.to_string_lossy(),
            &bundle_arg,
        ])
        .output()
        .expect("git bundle create");
    assert!(
        output.status.success(),
        "bundle create failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    // On the host side: fetch from the bundle
    let host_repo = Repository::open(host_repo_path).unwrap();
    let bundle_path_str = bundle_path.to_string_lossy().to_string();

    // Record old session branch head (if it exists) BEFORE updating
    let session_ref_name = format!("refs/heads/{}", session_branch);
    let old_session_oid = host_repo
        .find_reference(&session_ref_name)
        .ok()
        .and_then(|r| r.peel_to_commit().ok())
        .map(|c| c.id());

    // Fetch HEAD from bundle
    let fetch_output = std::process::Command::new("git")
        .args([
            "-C",
            &host_repo_path.to_string_lossy(),
            "fetch",
            &bundle_path_str,
            "HEAD",
        ])
        .output()
        .expect("git fetch");

    let used_fallback = if !fetch_output.status.success() {
        // Fallback: fetch all refs
        let fetch_all = std::process::Command::new("git")
            .args([
                "-C",
                &host_repo_path.to_string_lossy(),
                "fetch",
                &bundle_path_str,
                "+refs/*:refs/cc-bundle/*",
            ])
            .output()
            .expect("git fetch all");
        assert!(
            fetch_all.status.success(),
            "fetch all failed: {}",
            String::from_utf8_lossy(&fetch_all.stderr)
        );
        true
    } else {
        false
    };

    // Resolve FETCH_HEAD
    let fetch_head = host_repo.find_reference("FETCH_HEAD").unwrap();
    let fetch_commit = fetch_head.peel_to_commit().unwrap();
    let new_head_oid = fetch_commit.id();

    // Create/update the session branch
    host_repo
        .reference(
            &session_ref_name,
            new_head_oid,
            true,
            "cc: extract from container",
        )
        .unwrap();

    // Count commits: only NEW commits since old session branch head
    let commit_count = if let Some(old_oid) = old_session_oid {
        // Session branch existed before — count only new commits
        count_commits_between(&host_repo, old_oid, new_head_oid)
    } else {
        // First extract — count all reachable commits
        count_all_commits(&host_repo, new_head_oid)
    };

    // Clean up cc-bundle refs if we used the fallback
    if used_fallback {
        cleanup_bundle_refs(host_repo_path);
    }

    (commit_count, new_head_oid)
}

/// Count commits between `from` (exclusive) and `to` (inclusive).
fn count_commits_between(repo: &Repository, from: git2::Oid, to: git2::Oid) -> u32 {
    if from == to {
        return 0;
    }
    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push(to).unwrap();
    revwalk.hide(from).unwrap();
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL).unwrap();
    let mut count = 0u32;
    for oid in revwalk {
        if oid.is_ok() {
            count += 1;
        }
    }
    count
}

/// Count all reachable commits from `oid`.
fn count_all_commits(repo: &Repository, oid: git2::Oid) -> u32 {
    let mut revwalk = repo.revwalk().unwrap();
    revwalk.push(oid).unwrap();
    revwalk.set_sorting(git2::Sort::TOPOLOGICAL).unwrap();
    let mut count = 0u32;
    for oid in revwalk {
        if oid.is_ok() {
            count += 1;
            if count >= 10000 {
                break;
            }
        }
    }
    count
}

/// Delete all refs under refs/cc-bundle/.
fn cleanup_bundle_refs(repo_path: &Path) {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_path.to_string_lossy(),
            "for-each-ref",
            "--format=%(refname)",
            "refs/cc-bundle/",
        ])
        .output()
        .expect("for-each-ref");

    let refs = String::from_utf8_lossy(&output.stdout);
    for refname in refs.lines() {
        let refname = refname.trim();
        if !refname.is_empty() {
            let _ = std::process::Command::new("git")
                .args([
                    "-C",
                    &repo_path.to_string_lossy(),
                    "update-ref",
                    "-d",
                    refname,
                ])
                .output();
        }
    }
}

/// Check if any refs/cc-bundle/* refs exist in a repo.
fn has_bundle_refs(repo_path: &Path) -> bool {
    let output = std::process::Command::new("git")
        .args([
            "-C",
            &repo_path.to_string_lossy(),
            "for-each-ref",
            "--format=%(refname)",
            "refs/cc-bundle/",
        ])
        .output()
        .expect("for-each-ref");
    let refs = String::from_utf8_lossy(&output.stdout);
    refs.lines().any(|l| !l.trim().is_empty())
}

// ============================================================================
// Tests
// ============================================================================

/// First-time extract should count all commits.
#[test]
fn extract_first_time_counts_all() {
    let source = harness::TestRepo::new("extract-first-src");
    // initial commit = 1, then add 2 more = 3 total
    source.commit("second", &[("a.txt", "a")]);
    source.commit("third", &[("b.txt", "b")]);

    // Create a host repo that is a clone of source (simulates the host having the repo)
    let host_temp = tempfile::TempDir::new().unwrap();
    let host_path = host_temp.path().join("host-repo");
    Repository::clone(
        &source.path.to_string_lossy(),
        &host_path,
    )
    .unwrap();

    let (count, _oid) = simulate_extract_host_side(&source.path, &host_path, "test-session");

    // First extract: should count all 3 commits
    assert_eq!(count, 3, "first extract should count all reachable commits");
}

/// Second extract should count only new commits.
#[test]
fn extract_counts_only_new_commits() {
    let source = harness::TestRepo::new("extract-incr-src");
    // 1 (initial) + 4 = 5 commits
    source.commit("c2", &[("a.txt", "a")]);
    source.commit("c3", &[("b.txt", "b")]);
    source.commit("c4", &[("c.txt", "c")]);
    source.commit("c5", &[("d.txt", "d")]);

    // Clone to host
    let host_temp = tempfile::TempDir::new().unwrap();
    let host_path = host_temp.path().join("host-repo");
    Repository::clone(&source.path.to_string_lossy(), &host_path).unwrap();

    // First extract: session branch gets set to commit 5
    let (first_count, _) = simulate_extract_host_side(&source.path, &host_path, "test-session");
    assert_eq!(first_count, 5, "first extract: all 5 commits");

    // Now add more commits to the source
    source.commit("c6", &[("e.txt", "e")]);
    source.commit("c7", &[("f.txt", "f")]);
    source.commit("c8", &[("g.txt", "g")]);

    // Need to fetch new objects into host so the bundle can be resolved
    let fetch = std::process::Command::new("git")
        .args([
            "-C",
            &host_path.to_string_lossy(),
            "fetch",
            "origin",
        ])
        .output()
        .unwrap();
    // Fetch might fail if remote is a local path that doesn't auto-update;
    // instead re-clone or use bundle approach

    // Second extract: should count only the 3 new commits
    let (second_count, _) = simulate_extract_host_side(&source.path, &host_path, "test-session");
    assert_eq!(
        second_count, 3,
        "second extract should count only 3 new commits, not all 8"
    );
}

/// Bundle ref cleanup: after extract, no refs/cc-bundle/* should remain.
#[test]
fn extract_cleans_up_bundle_refs() {
    let source = harness::TestRepo::new("extract-cleanup-src");
    source.commit("c2", &[("a.txt", "content")]);

    let host_temp = tempfile::TempDir::new().unwrap();
    let host_path = host_temp.path().join("host-repo");
    Repository::clone(&source.path.to_string_lossy(), &host_path).unwrap();

    // Run the extract simulation
    let _ = simulate_extract_host_side(&source.path, &host_path, "test-session");

    // Verify no cc-bundle refs remain
    assert!(
        !has_bundle_refs(&host_path),
        "refs/cc-bundle/* should be cleaned up after extract"
    );
}

/// Targeted bundle: bundles only HEAD, not --all.
/// Verify bundle size is reasonable (not bloated with all refs).
#[test]
fn extract_targeted_bundle_is_smaller_than_all() {
    let source = harness::TestRepo::new("extract-targeted-src");
    // Create multiple branches to make --all noticeably different
    let repo = Repository::open(&source.path).unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();

    // Add commits on main
    for i in 0..5 {
        source.commit(
            &format!("main-{}", i),
            &[(&format!("main-{}.txt", i), &format!("content-{}", i))],
        );
    }

    // Create a side branch with its own commits
    let head = repo.head().unwrap().peel_to_commit().unwrap();
    repo.branch("side-branch", &head, false).unwrap();
    // Checkout side branch
    let side_ref = repo.find_reference("refs/heads/side-branch").unwrap();
    repo.set_head(side_ref.name().unwrap()).unwrap();
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    for i in 0..5 {
        let file_path = source.path.join(format!("side-{}.txt", i));
        std::fs::write(&file_path, format!("side-content-{}", i)).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new(&format!("side-{}.txt", i))).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let parent = repo.head().unwrap().peel_to_commit().unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, &format!("side-{}", i), &tree, &[&parent]).unwrap();
    }

    // Go back to main
    repo.set_head("refs/heads/master").ok()
        .or_else(|| repo.set_head("refs/heads/main").ok());
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    let bundle_dir = tempfile::tempdir().unwrap();

    // Bundle with --all
    let all_bundle = bundle_dir.path().join("all.bundle");
    let out_all = std::process::Command::new("git")
        .args([
            "-C",
            &source.path.to_string_lossy(),
            "bundle",
            "create",
            &all_bundle.to_string_lossy(),
            "--all",
        ])
        .output()
        .unwrap();
    assert!(out_all.status.success());

    // Bundle with HEAD only
    let head_bundle = bundle_dir.path().join("head.bundle");
    let out_head = std::process::Command::new("git")
        .args([
            "-C",
            &source.path.to_string_lossy(),
            "bundle",
            "create",
            &head_bundle.to_string_lossy(),
            "HEAD",
        ])
        .output()
        .unwrap();
    assert!(out_head.status.success());

    let all_size = std::fs::metadata(&all_bundle).unwrap().len();
    let head_size = std::fs::metadata(&head_bundle).unwrap().len();

    // HEAD-only bundle should be smaller (or equal if branches share all objects)
    // The key assertion: HEAD bundle should not be larger than --all bundle
    assert!(
        head_size <= all_size,
        "HEAD bundle ({} bytes) should be <= --all bundle ({} bytes)",
        head_size,
        all_size,
    );
}

/// Verify commit count is correct when session branch exists but points to same commit.
#[test]
fn extract_same_head_returns_zero_new_commits() {
    let source = harness::TestRepo::new("extract-same-head-src");
    source.commit("c2", &[("a.txt", "a")]);

    let host_temp = tempfile::TempDir::new().unwrap();
    let host_path = host_temp.path().join("host-repo");
    Repository::clone(&source.path.to_string_lossy(), &host_path).unwrap();

    // First extract
    let (first_count, _) = simulate_extract_host_side(&source.path, &host_path, "test-session");
    assert_eq!(first_count, 2);

    // Extract again with no new commits
    let (second_count, _) = simulate_extract_host_side(&source.path, &host_path, "test-session");
    assert_eq!(
        second_count, 0,
        "extracting same HEAD twice should report 0 new commits"
    );
}

// ============================================================================
// Docker-based integration tests (require running Docker)
// ============================================================================

#[tokio::test]
#[ignore]
async fn extract_via_docker_counts_only_new_commits() {
    // This would test the full SyncEngine::extract() with Docker.
    // Requires Docker to be running. Marked #[ignore].
    todo!("Docker integration test for extract commit counting");
}

#[tokio::test]
#[ignore]
async fn extract_via_docker_cleans_up_bundle_refs() {
    todo!("Docker integration test for bundle ref cleanup");
}

#[tokio::test]
#[ignore]
async fn extract_via_docker_handles_large_repo() {
    todo!("Docker integration test for large repo extract");
}
