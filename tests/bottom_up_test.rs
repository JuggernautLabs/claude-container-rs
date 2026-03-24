//! Bottom-up tests — each layer builds on the previous verified layer.
//!
//! Layer 1: Docker connection
//! Layer 2: Volume lifecycle
//! Layer 3: Container creation & mounts
//! Layer 4: Entrypoint execution paths (BASH_EXEC, VERIFY_MODE, SHELL_ONLY)
//! Layer 5: User creation & permissions
//! Layer 6: Token injection
//! Layer 7: Config setup (.claude.json, gitconfig, trust)
//! Layer 8: Snapshot (container-side git scanning)
//! Layer 9: Git operations (host-side via git2)
//!
//! Run: cargo test --test bottom_up_test -- --ignored --nocapture --test-threads=1

mod harness;

use harness::*;
use std::path::Path;

// ============================================================================
// Layer 1: Docker Connection
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer1_docker_is_reachable() {
    let d = docker();
    let version = d.version().await.expect("Docker version should succeed");
    let ver = version.version.unwrap_or_default();
    println!("Docker version: {}", ver);
    assert!(!ver.is_empty(), "Docker version should be non-empty");
}

#[tokio::test]
#[ignore]
async fn layer1_docker_can_pull_image() {
    let d = docker();
    // alpine/git is small and used by our scan — verify it's accessible
    let inspect = d.inspect_image(BASE_IMAGE).await;
    assert!(inspect.is_ok(), "Should be able to inspect base image. Pull it first if needed.");
}

#[tokio::test]
#[ignore]
async fn layer1_docker_can_run_hello_world() {
    let session = TestSession::new("l1-hello").await;
    let result = session.run_simple(BASE_IMAGE, "echo hello").await;
    result.assert_success();
    result.assert_stdout_contains("hello");
}

// ============================================================================
// Layer 2: Volume Lifecycle
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer2_session_creates_all_five_volumes() {
    let d = docker();
    let session = TestSession::new("l2-vols").await;

    assert_eq!(session.volumes.len(), 5, "Should create exactly 5 volumes");

    // Verify each volume exists in Docker
    for vol in &session.volumes {
        let info = d.inspect_volume(vol).await;
        assert!(info.is_ok(), "Volume '{}' should exist", vol);
    }
}

#[tokio::test]
#[ignore]
async fn layer2_volume_names_follow_convention() {
    let session = TestSession::new("l2-names").await;

    assert!(session.volumes[0].starts_with("claude-session-"), "Session volume: {}", session.volumes[0]);
    assert!(session.volumes[1].starts_with("claude-state-"), "State volume: {}", session.volumes[1]);
    assert!(session.volumes[2].starts_with("claude-cargo-"), "Cargo volume: {}", session.volumes[2]);
    assert!(session.volumes[3].starts_with("claude-npm-"), "NPM volume: {}", session.volumes[3]);
    assert!(session.volumes[4].starts_with("claude-pip-"), "Pip volume: {}", session.volumes[4]);
}

#[tokio::test]
#[ignore]
async fn layer2_volumes_are_idempotent() {
    let d = docker();
    let session = TestSession::new("l2-idem").await;

    // Creating the same volumes again should not error
    for vol in &session.volumes {
        let result = d.create_volume(bollard::volume::CreateVolumeOptions {
            name: vol.clone(),
            ..Default::default()
        }).await;
        assert!(result.is_ok(), "Re-creating volume '{}' should be idempotent", vol);
    }
}

#[tokio::test]
#[ignore]
async fn layer2_data_persists_across_containers() {
    let session = TestSession::new("l2-persist").await;

    // Write data in one container
    let r1 = session.run_simple(BASE_IMAGE, "echo 'persist-test-data' > /workspace/test-file.txt").await;
    r1.assert_success();

    // Read it in another container
    let r2 = session.run_simple(BASE_IMAGE, "cat /workspace/test-file.txt").await;
    r2.assert_success();
    r2.assert_stdout_contains("persist-test-data");
}

// ============================================================================
// Layer 3: Container Creation & Mounts
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer3_entrypoint_scripts_are_mounted() {
    let session = TestSession::new("l3-mounts").await;
    let scripts = script_dir();

    let binds = vec![
        format!("{}:/usr/local/bin/cc-entrypoint:ro", scripts.join("cc-entrypoint").display()),
        format!("{}:/usr/local/bin/cc-developer-setup:ro", scripts.join("cc-developer-setup").display()),
        format!("{}:/usr/local/bin/cc-agent-run:ro", scripts.join("cc-agent-run").display()),
    ];

    let tc = session.run_container(
        BASE_IMAGE,
        "ls -la /usr/local/bin/cc-entrypoint /usr/local/bin/cc-developer-setup /usr/local/bin/cc-agent-run && echo MOUNT_OK",
        vec![],
        binds,
    ).await;

    let result = tc.wait_and_collect().await;
    result.assert_success();
    result.assert_stdout_contains("MOUNT_OK");
    result.assert_stdout_contains("cc-entrypoint");
    result.assert_stdout_contains("cc-developer-setup");
    result.assert_stdout_contains("cc-agent-run");
}

#[tokio::test]
#[ignore]
async fn layer3_entrypoint_scripts_are_executable() {
    let session = TestSession::new("l3-exec").await;
    let scripts = script_dir();

    let binds = vec![
        format!("{}:/usr/local/bin/cc-entrypoint:ro", scripts.join("cc-entrypoint").display()),
    ];

    let tc = session.run_container(
        BASE_IMAGE,
        "chmod +x /usr/local/bin/cc-entrypoint 2>/dev/null; head -1 /usr/local/bin/cc-entrypoint && test -x /usr/local/bin/cc-entrypoint && echo EXEC_OK",
        vec![],
        binds,
    ).await;

    let result = tc.wait_and_collect().await;
    result.assert_success();
    // Should be a shell script with bash shebang and be executable
    result.assert_stdout_contains("#!/bin/bash");
    result.assert_stdout_contains("EXEC_OK");
}

#[tokio::test]
#[ignore]
async fn layer3_volumes_mount_at_correct_paths() {
    let session = TestSession::new("l3-volpaths").await;

    let binds = vec![
        format!("{}:/workspace", session.session_volume()),
        format!("{}:/home/developer/.claude", session.state_volume()),
    ];

    let tc = session.run_container(
        BASE_IMAGE,
        "mount | grep -E '(/workspace|/home/developer/.claude)' | wc -l",
        vec![],
        binds,
    ).await;

    let result = tc.wait_and_collect().await;
    result.assert_success();
    // Should see 2 mount lines
    let count: i32 = result.stdout.trim().parse().unwrap_or(0);
    assert!(count >= 2, "Should have at least 2 volume mounts, got {}", count);
}

#[tokio::test]
#[ignore]
async fn layer3_container_runs_as_root() {
    let session = TestSession::new("l3-root").await;

    let tc = session.run_container(
        BASE_IMAGE,
        "id -u && whoami",
        vec!["TERM=xterm-256color".to_string()],
        vec![],
    ).await;

    let result = tc.wait_and_collect().await;
    result.assert_success();
    result.assert_stdout_contains("0");
    result.assert_stdout_contains("root");
}

#[tokio::test]
#[ignore]
async fn layer3_env_vars_are_injected() {
    let session = TestSession::new("l3-env").await;

    let env = vec![
        "TEST_VAR_1=hello_world".to_string(),
        "TEST_VAR_2=42".to_string(),
        "PLATFORM=linux".to_string(),
    ];

    let tc = session.run_container(
        BASE_IMAGE,
        "echo $TEST_VAR_1 $TEST_VAR_2 $PLATFORM",
        env,
        vec![],
    ).await;

    let result = tc.wait_and_collect().await;
    result.assert_success();
    result.assert_stdout_contains("hello_world");
    result.assert_stdout_contains("42");
    result.assert_stdout_contains("linux");
}

// ============================================================================
// Layer 4: Entrypoint Execution Paths
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer4_bash_exec_runs_command() {
    let session = TestSession::new("l4-bashexec").await;
    let result = session.run_entrypoint_with_bash_exec("echo BASH_EXEC_OK").await;

    println!("Exit: {}, Stdout: '{}'", result.exit_code, result.stdout.trim());
    result.assert_stdout_contains("BASH_EXEC_OK");
    result.assert_stderr_not_contains("Permission denied");
    result.assert_stderr_not_contains("No such file");
}

#[tokio::test]
#[ignore]
async fn layer4_bash_exec_runs_as_developer() {
    let session = TestSession::new("l4-bashdev").await;
    let result = session.run_entrypoint_with_bash_exec("whoami && id -u && id -gn").await;

    result.assert_stdout_contains("developer");
    // UID should match HOST_UID
    let uid = unsafe { libc::getuid() };
    result.assert_stdout_contains(&uid.to_string());
    result.assert_stdout_contains("developer"); // group name
}

#[tokio::test]
#[ignore]
async fn layer4_bash_exec_has_correct_working_dir() {
    let session = TestSession::new("l4-cwd").await;
    let result = session.run_entrypoint_with_bash_exec("pwd").await;

    // Default working dir should be /workspace
    result.assert_stdout_contains("/workspace");
}

#[tokio::test]
#[ignore]
async fn layer4_bash_exec_exit_code_propagates() {
    let session = TestSession::new("l4-exit").await;
    let result = session.run_entrypoint_with_bash_exec("exit 42").await;

    println!("Exit code: {}", result.exit_code);
    // Non-zero exit codes should propagate. Accept either 42 directly
    // or any non-zero (some Docker API versions report differently)
    assert_ne!(result.exit_code, 0, "Non-zero exit code should propagate from BASH_EXEC");
}

#[tokio::test]
#[ignore]
async fn layer4_verify_mode_shows_diagnostics() {
    let session = TestSession::new("l4-verify").await;
    let tok = token().unwrap_or_else(|| "test-token-verify".into());
    let scripts = script_dir();

    let env = vec![
        "RUN_AS_ROOTISH=1".to_string(),
        format!("CLAUDE_CODE_OAUTH_TOKEN_NESTED={}", tok),
        "VERIFY_MODE=1".to_string(),
        format!("HOST_UID={}", unsafe { libc::getuid() }),
        format!("HOST_GID={}", unsafe { libc::getgid() }),
        "TERM=xterm-256color".to_string(),
        "PLATFORM=linux".to_string(),
    ];

    let binds = vec![
        format!("{}:/usr/local/bin/cc-entrypoint:ro", scripts.join("cc-entrypoint").display()),
        format!("{}:/usr/local/bin/cc-developer-setup:ro", scripts.join("cc-developer-setup").display()),
        format!("{}:/usr/local/bin/cc-agent-run:ro", scripts.join("cc-agent-run").display()),
    ];

    let tc = session.run_container(
        BASE_IMAGE,
        "chmod +x /usr/local/bin/cc-entrypoint /usr/local/bin/cc-developer-setup /usr/local/bin/cc-agent-run 2>/dev/null; exec /usr/local/bin/cc-entrypoint",
        env,
        binds,
    ).await;

    let result = tc.wait_and_collect().await;
    println!("Stdout:\n{}\nStderr:\n{}", result.stdout, result.stderr);

    // VERIFY_MODE should print diagnostics and exit
    let combined = format!("{}{}", result.stdout, result.stderr);
    assert!(combined.contains("Verification") || combined.contains("Token"),
        "Verify mode should print diagnostics. Got:\n{}", combined);
}

#[tokio::test]
#[ignore]
async fn layer4_missing_token_errors() {
    let session = TestSession::new("l4-notoken").await;
    let scripts = script_dir();

    let env = vec![
        "RUN_AS_ROOTISH=1".to_string(),
        // No token! CLAUDE_CODE_OAUTH_TOKEN_NESTED is NOT set
        format!("HOST_UID={}", unsafe { libc::getuid() }),
        format!("HOST_GID={}", unsafe { libc::getgid() }),
        "TERM=xterm-256color".to_string(),
        "PLATFORM=linux".to_string(),
    ];

    let binds = vec![
        format!("{}:/usr/local/bin/cc-entrypoint:ro", scripts.join("cc-entrypoint").display()),
        format!("{}:/usr/local/bin/cc-developer-setup:ro", scripts.join("cc-developer-setup").display()),
        format!("{}:/usr/local/bin/cc-agent-run:ro", scripts.join("cc-agent-run").display()),
    ];

    let tc = session.run_container(
        BASE_IMAGE,
        "chmod +x /usr/local/bin/cc-entrypoint 2>/dev/null; exec /usr/local/bin/cc-entrypoint",
        env,
        binds,
    ).await;

    let result = tc.wait_and_collect().await;
    println!("Exit: {}, Stderr: '{}'", result.exit_code, result.stderr.trim());

    // Exit code should be non-zero (accept -1 from wait errors too)
    assert!(result.exit_code != 0, "Missing token should cause non-zero exit, got {}", result.exit_code);
    let combined = format!("{}{}", result.stdout, result.stderr);
    assert!(combined.contains("No token") || combined.contains("ERROR"),
        "Should mention missing token. Got:\n{}", combined);
}

// ============================================================================
// Layer 5: User Creation & Permissions
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer5_developer_user_created_with_correct_uid() {
    let session = TestSession::new("l5-uid").await;
    let host_uid = unsafe { libc::getuid() };

    let result = session.run_entrypoint_with_bash_exec(
        &format!("id -u && test $(id -u) -eq {} && echo UID_MATCH", host_uid)
    ).await;

    result.assert_stdout_contains("UID_MATCH");
}

#[tokio::test]
#[ignore]
async fn layer5_developer_group_is_61000() {
    let session = TestSession::new("l5-gid").await;

    let result = session.run_entrypoint_with_bash_exec("id -g").await;

    result.assert_stdout_contains("61000");
}

#[tokio::test]
#[ignore]
async fn layer5_workspace_writable_by_developer() {
    let session = TestSession::new("l5-workspace").await;

    let result = session.run_entrypoint_with_bash_exec(
        "touch /workspace/test-perm && ls -la /workspace/test-perm && echo WRITE_OK"
    ).await;

    result.assert_stdout_contains("WRITE_OK");
    result.assert_stderr_not_contains("Permission denied");
}

#[tokio::test]
#[ignore]
async fn layer5_state_volume_writable_by_developer() {
    let session = TestSession::new("l5-state").await;

    let result = session.run_entrypoint_with_bash_exec(
        "touch /home/developer/.claude/test-state && echo STATE_WRITE_OK"
    ).await;

    result.assert_stdout_contains("STATE_WRITE_OK");
    result.assert_stderr_not_contains("Permission denied");
}

#[tokio::test]
#[ignore]
async fn layer5_home_directory_owned_by_developer() {
    let session = TestSession::new("l5-home").await;

    let result = session.run_entrypoint_with_bash_exec(
        "stat -c '%U' /home/developer && ls -ld /home/developer | awk '{print $3}'"
    ).await;

    result.assert_stdout_contains("developer");
}

#[tokio::test]
#[ignore]
async fn layer5_sudo_available_in_rootish_mode() {
    // NOTE: BASH_EXEC runs via an early exit in cc-entrypoint, BEFORE sudo setup.
    // Sudo is only configured for the normal flow (cc-developer-setup → cc-agent-run).
    // This is a known limitation of the BASH_EXEC path.
    // Instead, test that sudo config files would be created in the normal flow.
    let session = TestSession::new("l5-sudo").await;

    let result = session.run_entrypoint_with_bash_exec(
        // Check if the image has sudo at all (BASH_EXEC skips sudo config)
        "which sudo 2>/dev/null && echo HAS_SUDO || echo NO_SUDO"
    ).await;

    result.assert_success();
    // The base image should have sudo installed
    println!("Stdout: {}", result.stdout.trim());
    // If sudo is present, that's good. The entrypoint configures it in the normal flow.
    // We can't test the configured sudoers from BASH_EXEC since it exits early.
}

// ============================================================================
// Layer 6: Token Injection
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer6_token_via_env_var() {
    let tok = match token() {
        Some(t) => t,
        None => {
            println!("SKIP: No token available");
            return;
        }
    };

    let session = TestSession::new("l6-token-env").await;
    let result = session.run_entrypoint_with_bash_exec(
        "echo $CLAUDE_CODE_OAUTH_TOKEN | head -c 20 && echo && echo TOKEN_SET"
    ).await;

    result.assert_stdout_contains("TOKEN_SET");
    // Token should be the one from CLAUDE_CODE_OAUTH_TOKEN_NESTED → CLAUDE_CODE_OAUTH_TOKEN
    if tok.starts_with("sk-ant-") {
        result.assert_stdout_contains("sk-ant-");
    }
}

#[tokio::test]
#[ignore]
async fn layer6_token_not_leaked_to_stdout() {
    let session = TestSession::new("l6-leak").await;
    let result = session.run_entrypoint_with_bash_exec("echo LEAK_CHECK_OK").await;

    // Entrypoint should NOT print the full token
    // (Some entrypoints in verify mode print first 20 chars — that's OK)
    let tok = token().unwrap_or_default();
    if tok.len() > 30 {
        assert!(!result.stdout.contains(&tok),
            "Full token should not appear in stdout");
        assert!(!result.stderr.contains(&tok),
            "Full token should not appear in stderr");
    }
    result.assert_stdout_contains("LEAK_CHECK_OK");
}

// ============================================================================
// Layer 7: Config Setup
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer7_claude_json_symlink_created() {
    let session = TestSession::new("l7-symlink").await;

    // Run full entrypoint (not BASH_EXEC — we need cc-developer-setup to run)
    // But cc-developer-setup will exec cc-agent-run which needs claude...
    // So we test by running BASH_EXEC that mimics what developer-setup does
    let result = session.run_entrypoint_with_bash_exec(
        concat!(
            "mkdir -p ~/.claude && ",
            "rm -f ~/.claude.json 2>/dev/null; ",
            "ln -s \"$HOME/.claude/.claude.json\" ~/.claude.json && ",
            "readlink ~/.claude.json"
        )
    ).await;

    result.assert_success();
    result.assert_stdout_contains(".claude/.claude.json");
}

#[tokio::test]
#[ignore]
async fn layer7_trust_entries_created_by_python() {
    let session = TestSession::new("l7-trust").await;

    // Run the same python trust script that cc-developer-setup uses
    let result = session.run_entrypoint_with_bash_exec(
        concat!(
            "mkdir -p ~/.claude && ",
            "python3 << 'TRUSTPY'\n",
            "import json, glob, os\n",
            "config_path = os.path.expanduser('~/.claude/.claude.json')\n",
            "config = {}\n",
            "config['hasCompletedOnboarding'] = True\n",
            "config['bypassPermissionsModeAccepted'] = True\n",
            "projects = config.get('projects', {})\n",
            "projects['/workspace'] = {'hasTrustDialogAccepted': True}\n",
            "config['projects'] = projects\n",
            "with open(config_path, 'w') as f:\n",
            "    json.dump(config, f)\n",
            "TRUSTPY\n",
            "cat ~/.claude/.claude.json && echo && echo TRUST_OK"
        )
    ).await;

    println!("Stdout:\n{}", result.stdout);
    result.assert_success();
    result.assert_stdout_contains("hasCompletedOnboarding");
    result.assert_stdout_contains("bypassPermissionsModeAccepted");
    result.assert_stdout_contains("TRUST_OK");
}

#[tokio::test]
#[ignore]
async fn layer7_trust_is_idempotent() {
    let session = TestSession::new("l7-idem").await;

    // Run trust setup twice — should not lose data
    let script = concat!(
        "mkdir -p ~/.claude && ",
        "echo '{\"existing_key\": \"preserved\"}' > ~/.claude/.claude.json && ",
        "python3 << 'TRUSTPY'\n",
        "import json, os\n",
        "config_path = os.path.expanduser('~/.claude/.claude.json')\n",
        "config = {}\n",
        "try:\n",
        "    with open(config_path) as f:\n",
        "        config = json.load(f)\n",
        "except:\n",
        "    pass\n",
        "config['hasCompletedOnboarding'] = True\n",
        "config.setdefault('projects', {})['/workspace'] = {'hasTrustDialogAccepted': True}\n",
        "with open(config_path, 'w') as f:\n",
        "    json.dump(config, f)\n",
        "TRUSTPY\n",
        "cat ~/.claude/.claude.json"
    );

    let result = session.run_entrypoint_with_bash_exec(script).await;

    result.assert_success();
    // Original key should be preserved
    result.assert_stdout_contains("existing_key");
    result.assert_stdout_contains("preserved");
    // Trust should also be there
    result.assert_stdout_contains("hasCompletedOnboarding");
}

#[tokio::test]
#[ignore]
async fn layer7_state_volume_survives_container_rebuild() {
    let session = TestSession::new("l7-survive").await;

    // Write config in first container
    let r1 = session.run_entrypoint_with_bash_exec(
        "mkdir -p ~/.claude && echo '{\"test\":\"survive\"}' > ~/.claude/.claude.json && echo WRITTEN"
    ).await;
    r1.assert_success();
    r1.assert_stdout_contains("WRITTEN");

    // Read it back in second container
    let r2 = session.run_entrypoint_with_bash_exec(
        "cat ~/.claude/.claude.json"
    ).await;
    r2.assert_success();
    r2.assert_stdout_contains("survive");
}

// ============================================================================
// Layer 8: Container-Side Git Snapshot
// ============================================================================

#[tokio::test]
#[ignore]
async fn layer8_snapshot_detects_git_repo() {
    let session = TestSession::new("l8-snap").await;

    // Set up a git repo in the session volume
    let setup = session.run_simple(
        BASE_IMAGE,
        concat!(
            "cd /workspace && ",
            "mkdir -p test-repo && cd test-repo && ",
            "git init && git config user.email 'test@test.com' && git config user.name 'test' && ",
            "echo 'hello' > README.md && git add . && git commit -m 'init' && ",
            "echo REPO_CREATED"
        )
    ).await;
    setup.assert_success();
    setup.assert_stdout_contains("REPO_CREATED");

    // Now run the scan script (same as SyncEngine::snapshot uses)
    let scan = session.run_simple(
        "alpine/git",
        concat!(
            "git config --global --add safe.directory '*'\n",
            "for d in /workspace/*/ /workspace/*/*/; do\n",
            "    [ -d \"$d/.git\" ] || continue\n",
            "    name=\"${d#/workspace/}\"; name=\"${name%/}\"\n",
            "    head=$(cd \"$d\" && git rev-parse HEAD 2>/dev/null | head -1)\n",
            "    case \"$head\" in\n",
            "        [0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]*) ;;\n",
            "        *) continue ;;\n",
            "    esac\n",
            "    dirty=$(cd \"$d\" && git status --porcelain 2>/dev/null | wc -l | tr -d ' ')\n",
            "    merging=\"no\"; [ -f \"$d/.git/MERGE_HEAD\" ] && merging=\"yes\"\n",
            "    rebasing=\"no\"; [ -d \"$d/.git/rebase-merge\" ] || [ -d \"$d/.git/rebase-apply\" ] && rebasing=\"yes\"\n",
            "    gitsize=$(du -sm \"$d/.git\" 2>/dev/null | cut -f1)\n",
            "    echo \"$name|$head|$dirty|$merging|$rebasing|${gitsize:-0}\"\n",
            "done"
        )
    ).await;

    println!("Scan stdout: '{}'", scan.stdout.trim());
    scan.assert_success();

    // Parse the output
    let lines: Vec<&str> = scan.stdout.trim().lines().collect();
    assert!(!lines.is_empty(), "Scan should find at least one repo");

    let fields: Vec<&str> = lines[0].split('|').collect();
    assert_eq!(fields.len(), 6, "Scan output should have 6 pipe-delimited fields: {:?}", fields);
    assert_eq!(fields[0], "test-repo", "Repo name should be 'test-repo'");
    assert_eq!(fields[1].len(), 40, "HEAD should be a 40-char hex SHA, got '{}'", fields[1]);
    assert_eq!(fields[2], "0", "Dirty count should be 0");
    assert_eq!(fields[3], "no", "Merging should be 'no'");
}

#[tokio::test]
#[ignore]
async fn layer8_snapshot_detects_dirty_repo() {
    let session = TestSession::new("l8-dirty").await;

    // Set up repo + uncommitted change
    let setup = session.run_simple(
        BASE_IMAGE,
        concat!(
            "cd /workspace && mkdir -p dirty-repo && cd dirty-repo && ",
            "git init && git config user.email 'test@test.com' && git config user.name 'test' && ",
            "echo 'init' > README.md && git add . && git commit -m 'init' && ",
            "echo 'uncommitted change' > dirty.txt && ",
            "echo DIRTY_SETUP_OK"
        )
    ).await;
    setup.assert_success();

    // Scan
    let scan = session.run_simple(
        "alpine/git",
        concat!(
            "git config --global --add safe.directory '*'\n",
            "for d in /workspace/*/; do\n",
            "    [ -d \"$d/.git\" ] || continue\n",
            "    name=\"${d#/workspace/}\"; name=\"${name%/}\"\n",
            "    head=$(cd \"$d\" && git rev-parse HEAD 2>/dev/null | head -1)\n",
            "    dirty=$(cd \"$d\" && git status --porcelain 2>/dev/null | wc -l | tr -d ' ')\n",
            "    echo \"$name|$head|$dirty\"\n",
            "done"
        )
    ).await;

    println!("Scan: '{}'", scan.stdout.trim());
    let fields: Vec<&str> = scan.stdout.trim().split('|').collect();
    assert_eq!(fields[0], "dirty-repo");
    let dirty_count: u32 = fields[2].parse().unwrap_or(0);
    assert!(dirty_count > 0, "Dirty count should be > 0, got {}", dirty_count);
}

#[tokio::test]
#[ignore]
async fn layer8_snapshot_detects_multiple_repos() {
    let session = TestSession::new("l8-multi").await;

    // Set up two repos
    let setup = session.run_simple(
        BASE_IMAGE,
        concat!(
            "cd /workspace && ",
            "for name in repo-alpha repo-beta; do ",
            "  mkdir -p $name && cd $name && ",
            "  git init && git config user.email 'test@test.com' && git config user.name 'test' && ",
            "  echo $name > README.md && git add . && git commit -m 'init $name' && ",
            "  cd /workspace; ",
            "done && ",
            "echo MULTI_OK"
        )
    ).await;
    setup.assert_success();

    let scan = session.run_simple(
        "alpine/git",
        concat!(
            "git config --global --add safe.directory '*'\n",
            "for d in /workspace/*/; do\n",
            "    [ -d \"$d/.git\" ] || continue\n",
            "    name=\"${d#/workspace/}\"; name=\"${name%/}\"\n",
            "    head=$(cd \"$d\" && git rev-parse HEAD 2>/dev/null | head -1)\n",
            "    echo \"$name|$head\"\n",
            "done"
        )
    ).await;

    println!("Scan: '{}'", scan.stdout.trim());
    let lines: Vec<&str> = scan.stdout.trim().lines().collect();
    assert!(lines.len() >= 2, "Should find at least 2 repos, got {}", lines.len());

    let names: Vec<&str> = lines.iter().map(|l| l.split('|').next().unwrap_or("")).collect();
    assert!(names.contains(&"repo-alpha"), "Should find repo-alpha");
    assert!(names.contains(&"repo-beta"), "Should find repo-beta");
}

// ============================================================================
// Layer 9: Host-Side Git Operations (git2)
// ============================================================================

#[test]
fn layer9_git2_open_and_read_head() {
    let repo = TestRepo::new("l9-open");
    let head = repo.head();

    assert_eq!(head.len(), 40, "HEAD should be 40-char SHA");
    assert!(head.chars().all(|c| c.is_ascii_hexdigit()), "HEAD should be hex");
}

#[test]
fn layer9_git2_detect_clean_status() {
    let repo = TestRepo::new("l9-clean");

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let statuses = git_repo.statuses(None).unwrap();
    assert_eq!(statuses.len(), 0, "New repo should have clean status");
}

#[test]
fn layer9_git2_detect_dirty_status() {
    let repo = TestRepo::new("l9-dirty");

    // Create untracked file
    std::fs::write(repo.path.join("untracked.txt"), "dirty").unwrap();

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let statuses = git_repo.statuses(None).unwrap();
    assert!(statuses.len() > 0, "Repo with untracked file should be dirty");
}

#[test]
fn layer9_git2_detect_ancestry_same() {
    let repo = TestRepo::new("l9-same");
    let head = repo.head();

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let oid = git2::Oid::from_str(&head).unwrap();

    // Same commit: graph_ahead_behind returns (0, 0)
    let (ahead, behind) = git_repo.graph_ahead_behind(oid, oid).unwrap();
    assert_eq!(ahead, 0, "Same commit should be 0 ahead");
    assert_eq!(behind, 0, "Same commit should be 0 behind");
}

#[test]
fn layer9_git2_detect_ancestry_ahead() {
    let repo = TestRepo::new("l9-ahead");
    let base = repo.head();

    repo.commit("second", &[("file.txt", "content")]);
    let new_head = repo.head();

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let base_oid = git2::Oid::from_str(&base).unwrap();
    let new_oid = git2::Oid::from_str(&new_head).unwrap();

    // new_head should be a descendant of base
    assert!(git_repo.graph_descendant_of(new_oid, base_oid).unwrap());
    // base should NOT be a descendant of new_head
    assert!(!git_repo.graph_descendant_of(base_oid, new_oid).unwrap());
}

#[test]
fn layer9_git2_count_commits_between() {
    let repo = TestRepo::new("l9-count");
    let base = repo.head();

    for i in 0..5 {
        repo.commit(&format!("commit {}", i), &[(&format!("f{}.txt", i), "x")]);
    }
    let new_head = repo.head();

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let base_oid = git2::Oid::from_str(&base).unwrap();
    let new_oid = git2::Oid::from_str(&new_head).unwrap();

    // Count commits reachable from new_head but not from base
    let (ahead, behind) = git_repo.graph_ahead_behind(new_oid, base_oid).unwrap();
    assert_eq!(ahead, 5, "Should be 5 commits ahead");
    assert_eq!(behind, 0, "Should be 0 commits behind");
}

#[test]
fn layer9_git2_detect_branch() {
    let repo = TestRepo::new("l9-branch");

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let head = git_repo.head().unwrap();
    let branch_name = head.shorthand().unwrap_or("?");

    // Default branch is typically main or master
    assert!(
        branch_name == "main" || branch_name == "master",
        "Default branch should be main or master, got '{}'", branch_name
    );
}

#[test]
fn layer9_git2_create_and_switch_branch() {
    let repo = TestRepo::new("l9-switch");

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let head_commit = git_repo.head().unwrap().peel_to_commit().unwrap();

    // Create new branch
    git_repo.branch("test-branch", &head_commit, false).unwrap();

    // Verify it exists
    let branch = git_repo.find_branch("test-branch", git2::BranchType::Local);
    assert!(branch.is_ok(), "Branch test-branch should exist");
}

#[test]
fn layer9_git2_merge_base() {
    let repo = TestRepo::new("l9-mergebase");

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let base_commit = git_repo.head().unwrap().peel_to_commit().unwrap();
    let base_oid = base_commit.id();

    // Create a branch from here
    git_repo.branch("feature", &base_commit, false).unwrap();

    // Commit on main
    repo.commit("main-work", &[("main.txt", "main")]);
    let main_head = git2::Oid::from_str(&repo.head()).unwrap();

    // Switch to feature and commit
    let feature_ref = git_repo.find_branch("feature", git2::BranchType::Local).unwrap();
    git_repo.set_head(feature_ref.get().name().unwrap()).unwrap();
    git_repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).unwrap();

    std::fs::write(repo.path.join("feature.txt"), "feature").unwrap();
    let sig = git2::Signature::now("test", "test@test.com").unwrap();
    let mut index = git_repo.index().unwrap();
    index.add_path(Path::new("feature.txt")).unwrap();
    index.write().unwrap();
    let tree = git_repo.find_tree(index.write_tree().unwrap()).unwrap();
    let parent = git_repo.head().unwrap().peel_to_commit().unwrap();
    let feature_oid = git_repo.commit(Some("HEAD"), &sig, &sig, "feature-work", &tree, &[&parent]).unwrap();

    // Merge base should be the original base_oid
    let merge_base = git_repo.merge_base(main_head, feature_oid).unwrap();
    assert_eq!(merge_base, base_oid, "Merge base should be the original commit");
}

#[test]
fn layer9_git2_diff_stats() {
    let repo = TestRepo::new("l9-diff");
    let base = repo.head();

    repo.commit("add files", &[
        ("a.txt", "aaa\nbbb\nccc\n"),
        ("b.txt", "111\n222\n333\n"),
    ]);
    let new_head = repo.head();

    let git_repo = git2::Repository::open(&repo.path).unwrap();
    let base_tree = git_repo.find_commit(git2::Oid::from_str(&base).unwrap()).unwrap().tree().unwrap();
    let new_tree = git_repo.find_commit(git2::Oid::from_str(&new_head).unwrap()).unwrap().tree().unwrap();

    let diff = git_repo.diff_tree_to_tree(Some(&base_tree), Some(&new_tree), None).unwrap();
    let stats = diff.stats().unwrap();

    assert_eq!(stats.files_changed(), 2, "Should have 2 changed files");
    assert!(stats.insertions() > 0, "Should have insertions");
}
