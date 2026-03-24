//! Integration tests for Docker-dependent code.
//!
//! All tests are marked `#[ignore]` so they skip in CI.
//! Run with: `cargo test -- --ignored`
//!
//! Requires a running Docker daemon.

use std::path::{Path, PathBuf};
use std::process::Command;

use git_sandbox::lifecycle::Lifecycle;
use git_sandbox::session::SessionManager;
use git_sandbox::sync::SyncEngine;
use git_sandbox::types::docker::DockerState;
use git_sandbox::types::{ImageRef, SessionName};

// ============================================================================
// Helpers
// ============================================================================

/// Ensure DOCKER_HOST is set so bollard can find the socket.
/// On this machine the socket lives at ~/.colima/default/docker.sock.
fn ensure_docker_host() {
    if std::env::var("DOCKER_HOST").is_err() {
        let colima_sock = dirs::home_dir()
            .expect("home dir")
            .join(".colima/default/docker.sock");
        if colima_sock.exists() {
            std::env::set_var(
                "DOCKER_HOST",
                format!("unix://{}", colima_sock.display()),
            );
        }
        // else: fall through — connect_with_local_defaults will try /var/run/docker.sock
    }
}

fn docker_client() -> bollard::Docker {
    ensure_docker_host();
    bollard::Docker::connect_with_local_defaults().expect("Docker connection")
}

fn lifecycle() -> Lifecycle {
    ensure_docker_host();
    Lifecycle::new().expect("Lifecycle::new should succeed when Docker socket is reachable")
}

/// A test session that cleans up its Docker volumes on drop.
struct TestSession {
    name: SessionName,
}

impl TestSession {
    fn new(suffix: &str) -> Self {
        let pid = std::process::id();
        let name = SessionName::new(format!("rust-test-{pid}-{suffix}"));
        Self { name }
    }
}

impl Drop for TestSession {
    fn drop(&mut self) {
        let volumes = self.name.all_volumes();
        for vol in &volumes {
            let _ = Command::new("docker")
                .args(["volume", "rm", "-f", vol.as_str()])
                .output();
        }
        // Also remove any container that might have been created
        let ctr = self.name.container_name();
        let _ = Command::new("docker")
            .args(["rm", "-f", ctr.as_str()])
            .output();
    }
}

// ============================================================================
// 1. Lifecycle — Docker available
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_check_docker() {
    let lc = lifecycle();
    let state = lc.check_docker().await;
    assert!(
        matches!(state, DockerState::Available { .. }),
        "Expected DockerState::Available, got {:?}",
        state
    );
    if let DockerState::Available { version } = state {
        assert!(!version.is_empty(), "Docker version string should not be empty");
    }
}

// ============================================================================
// 2. Lifecycle — Image validation
// ============================================================================

/// The base claude-container image should have gosu, git, claude, bash.
/// Tries several known image names since the local tag varies.
#[tokio::test]
#[ignore]
async fn test_validate_base_image_passes() {
    let lc = lifecycle();

    let candidates = [
        "ghcr.io/hypermemetic/claude-container:latest",
        "claude-container-local:latest",
        "claude-container:latest",
    ];

    let mut validation = None;
    for name in &candidates {
        let image = ImageRef::new(*name);
        match lc.validate_image(&image).await {
            Ok(v) => {
                validation = Some((*name, v));
                break;
            }
            Err(_) => continue,
        }
    }

    let (name, v) = validation.expect(
        "No base claude-container image found. Tried: ghcr.io/hypermemetic/claude-container:latest, claude-container-local:latest, claude-container:latest"
    );
    assert!(
        v.is_valid(),
        "Image {} should pass validation. Missing critical: {:?}",
        name,
        v.missing_critical()
    );
}

/// alpine:latest should fail validation — it lacks gosu, git, claude.
#[tokio::test]
#[ignore]
async fn test_validate_alpine_fails() {
    let lc = lifecycle();
    let image = ImageRef::new("alpine:latest");
    let validation = lc.validate_image(&image).await;
    match validation {
        Ok(v) => {
            assert!(
                !v.is_valid(),
                "alpine:latest should NOT pass validation (missing gosu, claude, etc.)"
            );
            let missing = v.missing_critical();
            assert!(
                missing.contains(&"gosu"),
                "alpine:latest should be missing gosu, got missing: {:?}",
                missing
            );
            assert!(
                missing.contains(&"claude"),
                "alpine:latest should be missing claude, got missing: {:?}",
                missing
            );
        }
        Err(e) => {
            panic!(
                "validate_image failed (is alpine:latest pulled?): {}",
                e
            );
        }
    }
}

/// Second validation of the same image should hit the cache.
/// Verifies the cache directory is populated and a second call returns
/// the same result without error.
#[tokio::test]
#[ignore]
async fn test_validate_image_cached() {
    let lc = lifecycle();
    let image = ImageRef::new("alpine:latest");

    // First call — populates cache
    let first = lc
        .validate_image(&image)
        .await
        .expect("first validation should succeed");

    // Check cache directory exists and has entries
    let cache_dir = dirs::config_dir()
        .expect("config dir")
        .join("claude-container")
        .join("cache")
        .join("image-validated");

    assert!(
        cache_dir.exists(),
        "Validation cache directory should exist at {:?}",
        cache_dir
    );

    let entries: Vec<_> = std::fs::read_dir(&cache_dir)
        .expect("read cache dir")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        !entries.is_empty(),
        "Cache directory should have at least one entry after validation"
    );

    // Second call — should hit cache (no new container created)
    let second = lc
        .validate_image(&image)
        .await
        .expect("cached validation should succeed");

    // Results should be equivalent
    assert_eq!(
        first.missing_critical().len(),
        second.missing_critical().len(),
        "Cached result should match first result (critical)"
    );
    assert_eq!(
        first.missing_optional().len(),
        second.missing_optional().len(),
        "Cached result should match first result (optional)"
    );
}

// ============================================================================
// 3. Lifecycle — Volume operations
// ============================================================================

#[tokio::test]
#[ignore]
async fn test_create_and_check_volumes() {
    let ts = TestSession::new("vols");
    let lc = lifecycle();

    // Before creation — all should be missing
    let before = lc.check_volumes(&ts.name).await;
    assert!(
        !before.session.exists(),
        "Session volume should not exist before creation"
    );
    assert!(
        !before.state.exists(),
        "State volume should not exist before creation"
    );

    // Create volumes
    lc.create_volumes(&ts.name)
        .await
        .expect("create_volumes should succeed");

    // After creation — all should exist
    let after = lc.check_volumes(&ts.name).await;
    assert!(after.session.exists(), "Session volume should exist after creation");
    assert!(after.state.exists(), "State volume should exist after creation");
    assert!(after.cargo.exists(), "Cargo volume should exist after creation");
    assert!(after.npm.exists(), "Npm volume should exist after creation");
    assert!(after.pip.exists(), "Pip volume should exist after creation");
    assert!(after.all_exist(), "All volumes should exist");

    // Idempotent — creating again should not error
    lc.create_volumes(&ts.name)
        .await
        .expect("create_volumes should be idempotent");

    // TestSession::drop cleans up
}

// ============================================================================
// 4. Session — Discover existing session
// ============================================================================

/// Discover a known running session (synapse-cc-ux).
/// It may be Running or Stopped depending on current state, but should not
/// be DoesNotExist.
#[tokio::test]
#[ignore]
async fn test_discover_existing_session() {
    let docker = docker_client();
    let sm = SessionManager::new(docker);
    let name = SessionName::new("synapse-cc-ux");

    let discovered = sm.discover(&name).await.expect("discover should not error");
    assert!(
        !matches!(
            discovered,
            git_sandbox::types::DiscoveredSession::DoesNotExist(_)
        ),
        "synapse-cc-ux should exist (Running, Stopped, or VolumesOnly), got {:?}",
        discovered
    );
}

/// Discover a nonexistent session — should be DoesNotExist.
/// Uses a PID-based name to avoid collisions with leftover volumes.
#[tokio::test]
#[ignore]
async fn test_discover_nonexistent_session() {
    let docker = docker_client();
    let sm = SessionManager::new(docker);
    let pid = std::process::id();
    let name = SessionName::new(format!("nonexistent-{pid}-99999"));

    let discovered = sm.discover(&name).await.expect("discover should not error");
    assert!(
        matches!(
            discovered,
            git_sandbox::types::DiscoveredSession::DoesNotExist(_)
        ),
        "{} should be DoesNotExist, got {:?}",
        name,
        discovered
    );
}

// ============================================================================
// 5. Session — Read config
// ============================================================================

/// Read config from a known session — should have projects.
#[tokio::test]
#[ignore]
async fn test_read_config_existing_session() {
    let docker = docker_client();
    let sm = SessionManager::new(docker);
    let name = SessionName::new("synapse-cc-ux");

    let config = sm
        .read_config(&name)
        .await
        .expect("read_config should not error");

    match config {
        Some(cfg) => {
            assert!(
                !cfg.projects.is_empty(),
                "synapse-cc-ux config should have at least one project"
            );
        }
        None => {
            // Acceptable if the session volume has no .claude-projects.yml
            // (e.g. session exists but was created without config)
            eprintln!(
                "Warning: synapse-cc-ux has no .claude-projects.yml in its session volume"
            );
        }
    }
}

/// Read config from nonexistent session — should return None or error.
#[tokio::test]
#[ignore]
async fn test_read_config_nonexistent_session() {
    let docker = docker_client();
    let sm = SessionManager::new(docker);
    let pid = std::process::id();
    let name = SessionName::new(format!("nonexistent-{pid}-88888"));

    // This will fail at container creation (no volume), which should
    // surface as an error or None
    let result = sm.read_config(&name).await;
    match result {
        Ok(None) => {} // expected
        Ok(Some(_)) => panic!("Nonexistent session should not have a config"),
        Err(_) => {} // also acceptable — volume doesn't exist
    }
}

// ============================================================================
// 6. Session — Discover repos
// ============================================================================

/// Scan a known directory for git repos.
#[tokio::test]
#[ignore]
async fn test_discover_repos() {
    let docker = docker_client();
    let sm = SessionManager::new(docker);

    let scan_dir = Path::new("/Users/shmendez/dev/controlflow/hypermemetic");
    if !scan_dir.exists() {
        eprintln!("Skipping test_discover_repos: {} does not exist", scan_dir.display());
        return;
    }

    let repos = sm.discover_repos(scan_dir);
    assert!(
        !repos.is_empty(),
        "Should find at least one repo in {}",
        scan_dir.display()
    );

    for repo in &repos {
        let git_dir = repo.host_path.join(".git");
        assert!(
            git_dir.is_dir(),
            "Discovered repo {} should have a .git directory at {:?}",
            repo.name,
            git_dir
        );
    }

    // Verify they're sorted by name
    let names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "Repos should be sorted alphabetically");
}

// ============================================================================
// 7. Sync — Snapshot
// ============================================================================

/// Snapshot a known session — should return repos with valid commit hashes.
#[tokio::test]
#[ignore]
async fn test_snapshot_session() {
    // Clean up any leftover snapshot container from a previous run
    let _ = Command::new("docker")
        .args(["rm", "-f", "cc-snapshot-synapse-cc-ux"])
        .output();

    let docker = docker_client();
    let engine = SyncEngine::new(docker);
    let name = SessionName::new("synapse-cc-ux");

    let repos = engine
        .snapshot(&name, "main")
        .await
        .expect("snapshot should succeed");

    assert!(
        !repos.is_empty(),
        "synapse-cc-ux should have repos in its session volume"
    );

    for repo in &repos {
        assert!(
            !repo.name.is_empty(),
            "Each VolumeRepo should have a non-empty name"
        );
        assert!(
            repo.head.is_valid(),
            "Repo {} should have a valid commit hash, got '{}'",
            repo.name,
            repo.head.as_str()
        );
    }
}

// ============================================================================
// 8. Sync — Classify
// ============================================================================

/// Classify a repo that exists on both container and host side.
/// The container volume may store repos as flat names ("synapse") or nested
/// paths ("hypermemetic/synapse"), so we search multiple host roots.
///
/// Uses a different session from test_snapshot_session to avoid container
/// name conflicts when tests run in parallel.
#[tokio::test]
#[ignore]
async fn test_classify_repo() {
    let docker = docker_client();
    let engine = SyncEngine::new(docker);
    // Use synapse-dev (a different session from synapse-cc-ux used in
    // test_snapshot_session) to avoid container name collision when
    // tests run in parallel.
    let session = SessionName::new("synapse-dev");
    let _ = Command::new("docker")
        .args(["rm", "-f", &format!("cc-snapshot-{}", session)])
        .output();

    // First, snapshot to get container state
    let repos = engine
        .snapshot(&session, "main")
        .await
        .expect("snapshot should succeed");

    if repos.is_empty() {
        eprintln!("Skipping test_classify_repo: no repos in synapse-cc-ux snapshot");
        return;
    }

    // Search paths for host repos — repo names in the volume may be bare
    // directory names or nested paths
    let search_roots = [
        PathBuf::from("/Users/shmendez/dev/controlflow/hypermemetic"),
        PathBuf::from("/Users/shmendez/dev/controlflow"),
    ];

    let mut classified_any = false;

    for vr in &repos {
        // Try to locate this repo on the host
        let leaf_name = vr.name.split('/').last().unwrap_or(&vr.name);
        let mut host_path = None;

        for root in &search_roots {
            let candidate = root.join(&vr.name);
            if candidate.join(".git").is_dir() {
                host_path = Some(candidate);
                break;
            }
            let candidate = root.join(leaf_name);
            if candidate.join(".git").is_dir() {
                host_path = Some(candidate);
                break;
            }
        }

        let hp = match host_path {
            Some(p) => p,
            None => continue,
        };

        let pair = engine.classify_repo(
            &vr.name,
            vr,
            &hp,
            session.as_str(),
            "main",
        );

        // Container side should have a head (we got it from snapshot)
        assert!(
            pair.container.head().is_some(),
            "Container side of {} should have a HEAD",
            vr.name
        );

        // Host side should be present (we verified .git exists)
        assert!(
            pair.host.is_present(),
            "Host side of {} should be present at {:?}",
            vr.name,
            hp
        );

        // sync_decision should return a valid variant (not panic)
        let decision = pair.sync_decision();
        match &decision {
            git_sandbox::types::SyncDecision::Skip { .. }
            | git_sandbox::types::SyncDecision::Pull { .. }
            | git_sandbox::types::SyncDecision::Push { .. }
            | git_sandbox::types::SyncDecision::Reconcile { .. }
            | git_sandbox::types::SyncDecision::MergeToTarget { .. }
            | git_sandbox::types::SyncDecision::CloneToHost
            | git_sandbox::types::SyncDecision::PushToContainer
            | git_sandbox::types::SyncDecision::Blocked { .. } => {}
        }

        classified_any = true;
        break; // one is enough for the test
    }

    if !classified_any {
        let repo_names: Vec<&str> = repos.iter().map(|r| r.name.as_str()).collect();
        eprintln!(
            "Warning: could not find any snapshot repo on host. Volume repos: {:?}",
            repo_names
        );
    }
}
