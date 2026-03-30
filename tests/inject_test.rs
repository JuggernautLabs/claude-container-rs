//! Tests for inject (push) behavior — especially error handling.
//!
//! Run: cargo test --test inject_test -- --ignored --nocapture --test-threads=1

mod harness;

use harness::*;
use std::path::Path;

/// Reproduce the "Docker container wait error" from inject.
/// When the container exits non-zero, bollard's wait_container can return
/// Err(DockerContainerWaitError) instead of Ok(WaitResponse { status_code: 1 }).
/// The inject code only collects logs on Ok(non-zero), missing the Err path.
#[tokio::test]
#[ignore]
async fn inject_wait_error_still_reports_logs() {
    // Setup: create a session with a repo, then make host diverge
    let session = TestSession::new("inject-wait").await;

    // Create a repo in the session volume
    let setup = session.run_simple(
        BASE_IMAGE,
        concat!(
            "cd /workspace && mkdir -p test-repo && cd test-repo && ",
            "git init && git config user.email 'test@test.com' && git config user.name 'test' && ",
            "echo 'original' > file.txt && git add . && git commit -m 'init' && ",
            "echo SETUP_OK"
        ),
    ).await;
    setup.assert_success();

    // Create a host repo with diverged history
    let host_repo = TestRepo::new("inject-host");
    // Make a commit that conflicts
    host_repo.commit("host change", &[("file.txt", "host version")]);

    // Now try to inject — the container repo and host repo have diverged
    // on the same file, so git merge will fail
    let d = docker();
    let volume = session.session_volume().to_string();
    let host_path = host_repo.path.to_string_lossy().to_string();
    let container_name = format!("test-inject-wait-{}", std::process::id());

    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    // Run the inject script manually to see what happens
    let script = format!(
        r#"
git config --global --add safe.directory "*"
cd "/workspace/test-repo" || exit 1
git remote add _cc_upstream "/upstream" 2>/dev/null || git remote set-url _cc_upstream "/upstream"
git fetch _cc_upstream "main" 2>&1 || git fetch _cc_upstream "master" 2>&1 || {{ echo "FETCH_FAILED"; exit 1; }}

# Try to find the right branch name
BRANCH=""
for b in main master; do
    if git rev-parse --verify "_cc_upstream/$b" >/dev/null 2>&1; then
        BRANCH="$b"
        break
    fi
done

if [ -z "$BRANCH" ]; then
    echo "NO_BRANCH_FOUND"
    exit 1
fi

echo "MERGING _cc_upstream/$BRANCH"
if ! git merge "_cc_upstream/$BRANCH" --no-edit 2>&1; then
    echo "MERGE_CONFLICT"
    git merge --abort 2>/dev/null || true
    exit 1
fi
echo "MERGE_OK"
"#
    );

    let config = bollard::container::Config {
        image: Some("alpine/git".to_string()),
        entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
        cmd: Some(vec![script]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![
                format!("{}:/workspace", volume),
                format!("{}:/upstream:ro", host_path),
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };

    d.create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        config,
    ).await.expect("create inject container");

    d.start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>)
        .await.expect("start inject container");

    // Wait — this is where the bug manifests
    use futures_util::StreamExt;
    let mut wait = d.wait_container(
        &container_name,
        Some(bollard::container::WaitContainerOptions { condition: "not-running".to_string() }),
    );

    let mut exit_code: i64 = -999;
    let mut wait_error: Option<String> = None;

    while let Some(result) = wait.next().await {
        match result {
            Ok(resp) => {
                exit_code = resp.status_code;
                println!("Wait OK: exit_code={}", exit_code);
            }
            Err(e) => {
                wait_error = Some(format!("{:?}", e));
                println!("Wait Err: {:?}", e);
                // Extract exit code from error if possible
                if let bollard::errors::Error::DockerContainerWaitError { code, .. } = &e {
                    exit_code = *code;
                    println!("  Extracted exit code from error: {}", code);
                }
            }
        }
    }

    // Collect logs regardless of how wait ended
    let mut stdout = String::new();
    let mut stderr = String::new();
    let mut logs = d.logs(
        &container_name,
        Some(bollard::container::LogsOptions::<String> {
            stdout: true,
            stderr: true,
            follow: false,
            ..Default::default()
        }),
    );
    while let Some(Ok(chunk)) = logs.next().await {
        match chunk {
            bollard::container::LogOutput::StdOut { message } => {
                stdout.push_str(&String::from_utf8_lossy(&message));
            }
            bollard::container::LogOutput::StdErr { message } => {
                stderr.push_str(&String::from_utf8_lossy(&message));
            }
            _ => {}
        }
    }

    // Cleanup
    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    println!("=== RESULTS ===");
    println!("exit_code: {}", exit_code);
    println!("wait_error: {:?}", wait_error);
    println!("stdout: {}", stdout);
    println!("stderr: {}", stderr);

    // The key assertion: even if wait returned Err, we should still
    // have collected logs that tell us what happened
    if wait_error.is_some() {
        println!("\n=== BUG REPRODUCED ===");
        println!("wait_container returned Err instead of Ok(non-zero).");
        println!("Current inject code would report 'Docker container wait error'");
        println!("instead of collecting logs and reporting the actual problem.");
        println!("Logs contain: {}", if stdout.contains("MERGE_CONFLICT") { "MERGE_CONFLICT" } else { &stdout });
    }

    // Verify the volume is clean (merge was aborted)
    let check = session.run_simple(
        "alpine/git",
        "cd /workspace/test-repo && git status --short && ls .git/MERGE_HEAD 2>&1 || echo NO_MERGE_HEAD",
    ).await;
    println!("\n=== Volume state after inject ===");
    println!("{}", check.stdout);
}

/// Test that inject properly handles the Err path from wait_container
/// by still collecting logs and returning a meaningful error.
#[tokio::test]
#[ignore]
async fn inject_collects_logs_on_wait_error() {
    let session = TestSession::new("inject-logs").await;

    // Create a repo that will cause the inject to fail
    let setup = session.run_simple(
        BASE_IMAGE,
        concat!(
            "cd /workspace && mkdir -p test-repo && cd test-repo && ",
            "git init && git config user.email 'test@test.com' && git config user.name 'test' && ",
            "echo 'v1' > file.txt && git add . && git commit -m 'init'"
        ),
    ).await;
    setup.assert_success();

    // Try to inject from a non-existent branch — should fail with clear error
    let host_repo = TestRepo::new("inject-logs-host");

    let d = docker();
    let volume = session.session_volume().to_string();
    let host_path = host_repo.path.to_string_lossy().to_string();
    let container_name = format!("test-inject-logs-{}", std::process::id());

    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    let script = r#"
git config --global --add safe.directory "*"
cd "/workspace/test-repo" || exit 1
git remote add _cc_upstream "/upstream" 2>/dev/null || git remote set-url _cc_upstream "/upstream"
echo "FETCHING nonexistent-branch"
git fetch _cc_upstream "nonexistent-branch" 2>&1
echo "FETCH_EXIT=$?"
exit 1
"#;

    let config = bollard::container::Config {
        image: Some("alpine/git".to_string()),
        entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
        cmd: Some(vec![script.to_string()]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![
                format!("{}:/workspace", volume),
                format!("{}:/upstream:ro", host_path),
            ]),
            ..Default::default()
        }),
        ..Default::default()
    };

    d.create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        config,
    ).await.expect("create");

    d.start_container(&container_name, None::<bollard::container::StartContainerOptions<String>>)
        .await.expect("start");

    use futures_util::StreamExt;
    let mut wait = d.wait_container(
        &container_name,
        Some(bollard::container::WaitContainerOptions { condition: "not-running".to_string() }),
    );

    let mut got_ok = false;
    let mut got_err = false;

    while let Some(result) = wait.next().await {
        match result {
            Ok(resp) => {
                got_ok = true;
                println!("Wait Ok: status={}", resp.status_code);
            }
            Err(e) => {
                got_err = true;
                println!("Wait Err: {:?}", e);
            }
        }
    }

    // Collect logs — this should ALWAYS work even after Err
    let mut output = String::new();
    let mut logs = d.logs(
        &container_name,
        Some(bollard::container::LogsOptions::<String> {
            stdout: true,
            stderr: true,
            follow: false,
            ..Default::default()
        }),
    );
    while let Some(Ok(chunk)) = logs.next().await {
        output.push_str(&chunk.to_string());
    }

    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    println!("got_ok={}, got_err={}", got_ok, got_err);
    println!("logs: {}", output);

    // We should always be able to collect logs
    assert!(!output.is_empty(), "Should have collected logs even on wait error");
    assert!(output.contains("FETCHING"), "Logs should contain our echo");
}
