//! Integration tests for start/resume behavior.
//! Run: cargo test --test launch_path_test -- --ignored --nocapture --test-threads=1

mod harness;
use harness::*;

fn find_binary() -> std::path::PathBuf {
    [
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/debug/gitvm"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("target/release/gitvm"),
        dirs::home_dir().unwrap_or_default().join(".cargo/bin/gitvm"),
    ].into_iter().find(|p| p.exists()).expect("build gitvm first")
}

async fn cleanup(d: &bollard::Docker, name: &str) {
    let ctr = format!("claude-session-ctr-{}", name);
    let _ = d.remove_container(&ctr, Some(bollard::container::RemoveContainerOptions {
        force: true, ..Default::default()
    })).await;
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, name);
        let _ = d.remove_volume(&vol, None::<bollard::volume::RemoveVolumeOptions>).await;
    }
    let meta = dirs::home_dir().unwrap_or_default()
        .join(format!(".config/claude-container/sessions/{}.env", name));
    let _ = std::fs::remove_file(&meta);
}

fn run_with_timeout(args: &[&str], timeout_secs: u64) -> (String, String, i32) {
    let binary = find_binary();
    let mut cmd_args: Vec<&str> = vec!["-y"];
    cmd_args.extend_from_slice(args);
    let output = std::process::Command::new("timeout")
        .arg(format!("{}", timeout_secs))
        .arg(&binary)
        .args(&cmd_args)
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .current_dir(std::env::temp_dir())
        .output()
        .expect("failed to run");
    (
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
        output.status.code().unwrap_or(-1),
    )
}

#[tokio::test]
#[ignore]
async fn start_no_repo_exits_cleanly() {
    let d = docker();
    let name = format!("lp-norepo-{}", std::process::id());
    cleanup(&d, &name).await;

    let (_, stderr, code) = run_with_timeout(&["start", "-s", &name], 10);
    cleanup(&d, &name).await;

    assert_ne!(code, 0, "Should fail");
    assert_ne!(code, 124, "Should not hang");
    assert!(stderr.contains("No repos"), "Got:\n{}", stderr);
}

#[tokio::test]
#[ignore]
async fn start_running_warns_without_attach() {
    let d = docker();
    let name = format!("lp-running-{}", std::process::id());
    cleanup(&d, &name).await;

    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, name);
        let _ = d.create_volume(bollard::volume::CreateVolumeOptions {
            name: vol, ..Default::default()
        }).await;
    }

    let ctr = format!("claude-session-ctr-{}", name);
    d.create_container(
        Some(bollard::container::CreateContainerOptions { name: ctr.as_str(), platform: None }),
        bollard::container::Config {
            image: Some(BASE_IMAGE.to_string()),
            cmd: Some(vec!["sleep".to_string(), "30".to_string()]),
            ..Default::default()
        },
    ).await.expect("create");
    d.start_container(&ctr, None::<bollard::container::StartContainerOptions<String>>).await.expect("start");

    let (_, stderr, code) = run_with_timeout(&["start", "-s", &name], 10);
    cleanup(&d, &name).await;

    assert_ne!(code, 124, "Should not hang");
    assert!(stderr.contains("already running") && stderr.contains("-a"),
        "Got:\n{}", stderr);
}

#[tokio::test]
#[ignore]
async fn no_staircase_in_output() {
    let d = docker();
    let name = format!("lp-stair-{}", std::process::id());
    cleanup(&d, &name).await;

    let (_, stderr, _) = run_with_timeout(&["start", "-s", &name], 10);
    cleanup(&d, &name).await;

    for (i, line) in stderr.lines().enumerate() {
        let leading = line.len() - line.trim_start().len();
        assert!(leading < 10, "Line {} staircase: '{}'", i, line);
    }
}
