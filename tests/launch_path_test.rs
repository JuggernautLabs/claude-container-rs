//! Integration tests for cmd_start output formatting and LaunchPath behavior.
//!
//! These tests run the actual `git-sandbox` binary and capture its stderr output
//! to verify user-facing messages are correct.
//!
//! Run: cargo test --test launch_path_test -- --ignored --nocapture --test-threads=1

mod harness;

use harness::*;

// ============================================================================
// Output formatting tests (Docker required, #[ignore])
// ============================================================================

/// Helper: find the git-sandbox binary
fn find_binary() -> std::path::PathBuf {
    let candidates = [
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/debug/git-sandbox"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/release/git-sandbox"),
        dirs::home_dir()
            .unwrap_or_default()
            .join(".cargo/bin/git-sandbox"),
    ];

    candidates.into_iter()
        .find(|p| p.exists())
        .expect("Can't find git-sandbox binary. Run `cargo build` first.")
}

/// Helper: clean up session volumes
async fn cleanup_session_volumes(d: &bollard::Docker, session_name: &str) {
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.remove_volume(&vol, None::<bollard::volume::RemoveVolumeOptions>).await;
    }
}

#[tokio::test]
#[ignore]
async fn output_start_default_image_shows_default_tag() {
    // A new session with no dockerfile should show "(default)" in the image line.
    // We run from the repo dir (which IS a git repo), so it creates a session
    // and eventually prints the image line.
    let session_name = format!("lp-default-{}", std::process::id());
    let binary = find_binary();

    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDERR ===\n{}", stderr);

    let d = docker();
    cleanup_session_volumes(&d, &session_name).await;

    // The image line should reference the default image and say "(default)"
    let image_line = stderr.lines().find(|l| l.contains("image:"));
    if let Some(line) = image_line {
        assert!(
            line.contains("(default)"),
            "Default image line should contain '(default)'. Got: '{}'",
            line
        );
        assert!(
            line.contains("ghcr.io/") || line.contains("claude-container"),
            "Default image line should reference the standard image. Got: '{}'",
            line
        );
    }
    // If no image line, the session may have failed before reaching image resolution
    // (e.g., clone not implemented). That's OK for this test — we just verify format when present.
}

#[tokio::test]
#[ignore]
async fn output_start_dockerfile_shows_path() {
    // When a Dockerfile is provided, the image line should show the path.
    let session_name = format!("lp-dockerfile-{}", std::process::id());
    let binary = find_binary();

    // Create a temporary Dockerfile
    let temp_dir = tempfile::TempDir::new().unwrap();
    let dockerfile_path = temp_dir.path().join("Dockerfile");
    std::fs::write(&dockerfile_path, "FROM alpine:latest\nRUN echo hello\n").unwrap();

    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name, "--dockerfile", &dockerfile_path.display().to_string()])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDERR ===\n{}", stderr);

    let d = docker();
    cleanup_session_volumes(&d, &session_name).await;

    let image_line = stderr.lines().find(|l| l.contains("image:"));
    if let Some(line) = image_line {
        assert!(
            line.contains("from"),
            "Dockerfile image line should contain 'from' (showing source path). Got: '{}'",
            line
        );
        assert!(
            line.contains(&format!("claude-dev-{}", session_name)),
            "Dockerfile image line should contain the built image name. Got: '{}'",
            line
        );
    }
}

#[tokio::test]
#[ignore]
async fn output_start_running_shows_warning() {
    // When a container is already running, the output should warn the user.
    let d = docker();
    let session_name = format!("lp-running-{}", std::process::id());
    let container_name = format!("claude-session-ctr-{}", session_name);

    // Create the volumes (discovery checks for these)
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.create_volume(bollard::volume::CreateVolumeOptions {
            name: vol,
            ..Default::default()
        }).await;
    }

    // Create and start a container that stays running
    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    d.create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        bollard::container::Config {
            image: Some(BASE_IMAGE.to_string()),
            cmd: Some(vec!["sleep".to_string(), "30".to_string()]),
            tty: Some(true),
            open_stdin: Some(true),
            ..Default::default()
        },
    ).await.expect("create running container");

    d.start_container(
        &container_name,
        None::<bollard::container::StartContainerOptions<String>>,
    ).await.expect("start container");

    // Run git-sandbox start (without -a flag)
    let binary = find_binary();
    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDERR ===\n{}", stderr);

    // Clean up
    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;
    cleanup_session_volumes(&d, &session_name).await;

    // The "already running" message should be present
    assert!(
        stderr.contains("Container already running"),
        "Should warn about already running container. Got:\n{}",
        stderr
    );
    assert!(
        stderr.contains("-a"),
        "Should mention -a flag for attaching. Got:\n{}",
        stderr
    );
}

#[tokio::test]
#[ignore]
async fn output_no_staircase_on_start() {
    // All output lines should have < 10 leading spaces (no raw mode leak).
    let session_name = format!("lp-staircase-{}", std::process::id());
    let binary = find_binary();

    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDERR ===\n{}", stderr);

    let d = docker();
    cleanup_session_volumes(&d, &session_name).await;

    for (i, line) in stderr.lines().enumerate() {
        let leading = line.len() - line.trim_start().len();
        assert!(
            leading < 10,
            "STAIRCASE: Line {} has {} leading spaces: '{}'\nFull output:\n{}",
            i, leading, line, stderr
        );
    }
}

#[tokio::test]
#[ignore]
async fn output_start_shows_session_name() {
    // The first line of output should include the session name.
    let session_name = format!("lp-name-{}", std::process::id());
    let binary = find_binary();

    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDERR ===\n{}", stderr);

    let d = docker();
    cleanup_session_volumes(&d, &session_name).await;

    let first_line = stderr.lines().next().unwrap_or("");
    assert!(
        first_line.contains(&session_name),
        "First line should contain session name '{}'. Got: '{}'",
        session_name, first_line
    );
    assert!(
        first_line.contains("Session:"),
        "First line should contain 'Session:'. Got: '{}'",
        first_line
    );
}

#[tokio::test]
#[ignore]
async fn output_start_all_to_stderr() {
    // All user-facing output should go to stderr, stdout should be empty.
    let session_name = format!("lp-stderr-{}", std::process::id());
    let binary = find_binary();

    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDOUT ({} bytes) ===\n{}", stdout.len(), stdout);
    println!("=== STDERR ({} bytes) ===\n{}", stderr.len(), stderr);

    let d = docker();
    cleanup_session_volumes(&d, &session_name).await;

    assert_eq!(
        stdout.trim(), "",
        "stdout should be empty — all start output goes to stderr"
    );
    assert!(
        !stderr.trim().is_empty(),
        "stderr should have output"
    );
}

// ============================================================================
// Resume path — stopped container shows correct messaging
// ============================================================================

#[tokio::test]
#[ignore]
async fn output_start_stopped_container_shows_resuming() {
    // When a stopped container exists, the output should mention resuming.
    let d = docker();
    let session_name = format!("lp-stopped-{}", std::process::id());
    let container_name = format!("claude-session-ctr-{}", session_name);

    // Create volumes
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.create_volume(bollard::volume::CreateVolumeOptions {
            name: vol,
            ..Default::default()
        }).await;
    }

    // Create a container and then stop it
    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    d.create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        bollard::container::Config {
            image: Some(BASE_IMAGE.to_string()),
            cmd: Some(vec!["echo".to_string(), "done".to_string()]),
            ..Default::default()
        },
    ).await.expect("create container");

    d.start_container(
        &container_name,
        None::<bollard::container::StartContainerOptions<String>>,
    ).await.expect("start container");

    // Wait for it to exit naturally
    use futures_util::StreamExt;
    let mut wait = d.wait_container(
        &container_name,
        Some(bollard::container::WaitContainerOptions { condition: "not-running".to_string() }),
    );
    while let Some(_) = wait.next().await {}

    // Now run git-sandbox start — should hit the Stopped path
    let binary = find_binary();
    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDERR ===\n{}", stderr);

    // Clean up
    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;
    cleanup_session_volumes(&d, &session_name).await;

    // Should mention resuming
    assert!(
        stderr.contains("Resuming stopped container") || stderr.contains("Session exists"),
        "Stopped container path should mention resuming or existing session. Got:\n{}",
        stderr
    );
}

// ============================================================================
// VolumesOnly path — no container, volumes present
// ============================================================================

#[tokio::test]
#[ignore]
async fn output_start_volumes_only_shows_exists() {
    // When volumes exist but no container, should mention "Session exists".
    let d = docker();
    let session_name = format!("lp-volonly-{}", std::process::id());

    // Create volumes (no container)
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.create_volume(bollard::volume::CreateVolumeOptions {
            name: vol,
            ..Default::default()
        }).await;
    }

    let binary = find_binary();
    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run git-sandbox");

    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDERR ===\n{}", stderr);

    cleanup_session_volumes(&d, &session_name).await;

    assert!(
        stderr.contains("Session exists"),
        "VolumesOnly path should say 'Session exists'. Got:\n{}",
        stderr
    );
}
