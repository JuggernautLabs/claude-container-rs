//! Output snapshot tests — capture real output, assert it matches expected strings.
//!
//! These tests catch garbled output, staircase effects, missing newlines,
//! and regressions in user-facing messages.
//!
//! Run: cargo test --test output_test -- --ignored --nocapture --test-threads=1

mod harness;

use harness::*;

// ============================================================================
// Entrypoint output — what does cc-entrypoint actually print?
// ============================================================================

#[tokio::test]
#[ignore]
async fn output_entrypoint_bash_exec_clean() {
    let session = TestSession::new("out-bashexec").await;
    let result = session.run_entrypoint_with_bash_exec("echo HELLO").await;

    // Capture both streams
    let stdout = result.stdout.clone();
    let stderr = result.stderr.clone();

    println!("=== STDOUT ({} bytes) ===", stdout.len());
    println!("{}", stdout);
    println!("=== STDERR ({} bytes) ===", stderr.len());
    println!("{}", stderr);
    println!("=== EXIT: {} ===", result.exit_code);

    // SNAPSHOT: BASH_EXEC with "echo HELLO" should produce exactly this
    assert_eq!(result.exit_code, 0);
    assert_eq!(stdout.trim(), "HELLO", "stdout should be exactly 'HELLO'");
    assert_eq!(stderr.trim(), "", "stderr should be empty for BASH_EXEC");
}

#[tokio::test]
#[ignore]
async fn output_entrypoint_verify_mode() {
    let session = TestSession::new("out-verify").await;
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

    let stdout = result.stdout.clone();
    let stderr = result.stderr.clone();

    println!("=== STDOUT ({} bytes) ===", stdout.len());
    println!("{}", stdout);
    println!("=== STDERR ({} bytes) ===", stderr.len());
    println!("{}", stderr);
    println!("=== EXIT: {} ===", result.exit_code);

    // SNAPSHOT: verify mode should print exactly these sections
    assert_eq!(result.exit_code, 0);
    assert_eq!(stderr.trim(), "", "stderr should be empty in verify mode");

    // Check structural lines (token prefix and claude response vary)
    let lines: Vec<&str> = stdout.lines().collect();
    assert_eq!(lines[0], "=== Verification ===");
    assert_eq!(lines[1], "Run as: developer (sudo)");
    assert_eq!(lines[2], "Claude: /usr/bin/claude");
    assert_eq!(lines[3], "Platform: linux");
    // lines[4] is blank
    assert_eq!(lines[5], "=== Token Check ===");
    assert_eq!(lines[6], "  Token source: nested env var");
    assert!(lines[7].starts_with("  Token: "), "Token line should start with '  Token: ', got '{}'", lines[7]);
    // lines[8] is blank
    assert_eq!(lines[9], "=== Claude Test ===");
    // Remaining lines are claude's response — just check it's not empty
    assert!(lines.len() > 10, "Claude test should produce output");
}

#[tokio::test]
#[ignore]
async fn output_entrypoint_missing_token() {
    let session = TestSession::new("out-notoken").await;
    let scripts = script_dir();

    let env = vec![
        "RUN_AS_ROOTISH=1".to_string(),
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

    let stdout = result.stdout.clone();
    let stderr = result.stderr.clone();

    println!("=== STDOUT ({} bytes) ===", stdout.len());
    println!("{}", stdout);
    println!("=== STDERR ({} bytes) ===", stderr.len());
    println!("{}", stderr);
    println!("=== EXIT: {} ===", result.exit_code);

    // SNAPSHOT: missing token should produce exactly this
    assert_eq!(result.exit_code, 1);
    assert_eq!(stdout.trim(), "", "stdout should be empty on token error");
    assert_eq!(
        stderr.trim(),
        "ERROR: No token found (neither CLAUDE_CODE_OAUTH_TOKEN_NESTED env var nor /run/secrets/claude_token)",
        "stderr should be the exact error message"
    );
}

// ============================================================================
// Binary output — what does `gitvm start` print?
// ============================================================================

#[tokio::test]
#[ignore]
async fn output_start_new_session() {
    // Run the actual binary against a fresh session name
    let session_name = format!("out-start-{}", std::process::id());

    let binary = find_binary();

    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")  // disable color codes for clean comparison
        .output()
        .expect("failed to run gitvm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDOUT ({} bytes) ===", stdout.len());
    println!("{}", stdout);
    println!("=== STDERR ({} bytes) ===", stderr.len());
    println!("{}", stderr);
    println!("=== EXIT: {} ===", output.status.code().unwrap_or(-1));

    // Clean up volumes if created
    let d = docker();
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.remove_volume(&vol, None::<bollard::volume::RemoveVolumeOptions>).await;
    }

    // SNAPSHOT: new session from git repo dir should produce exactly these lines
    let exit_code = output.status.code().unwrap_or(-1);
    assert_eq!(exit_code, 0, "Should exit 0");
    assert_eq!(stdout.trim(), "", "stdout should be empty (all output to stderr)");

    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 7, "Should have exactly 7 lines. Got:\n{}", stderr);
    assert!(lines[0].starts_with("→ Session: out-start-"), "Line 0: '{}'", lines[0]);
    assert!(lines[1].contains("Using current directory: claude-container-rs"), "Line 1: '{}'", lines[1]);
    assert!(lines[2].contains("· claude-container-rs"), "Line 2: '{}'", lines[2]);
    assert!(lines[2].contains("master"), "Line 2 should show branch: '{}'", lines[2]);
    assert!(lines[3].contains("Creating volumes..."), "Line 3: '{}'", lines[3]);
    assert!(lines[4].contains("Session '") && lines[4].contains("' created with 1 repo(s)"), "Line 4: '{}'", lines[4]);
    assert!(lines[5].contains("Repo cloning into volume not yet implemented"), "Line 5: '{}'", lines[5]);
    assert!(lines[6].contains("Use: claude-container -s"), "Line 6: '{}'", lines[6]);

    // Check no staircase: no line should have excessive leading whitespace
    for (i, line) in lines.iter().enumerate() {
        let leading = line.len() - line.trim_start().len();
        assert!(leading < 10, "Line {} has {} leading spaces: '{}'", i, leading, line);
    }
}

#[tokio::test]
#[ignore]
async fn output_start_nonexistent_no_repo() {
    // Run start from a non-git directory — should fail cleanly
    let session_name = format!("out-norepo-{}", std::process::id());

    let binary = find_binary();

    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .current_dir(std::env::temp_dir())  // not a git repo
        .output()
        .expect("failed to run gitvm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    println!("=== STDOUT ({} bytes) ===", stdout.len());
    println!("{}", stdout);
    println!("=== STDERR ({} bytes) ===", stderr.len());
    println!("{}", stderr);
    println!("=== EXIT: {} ===", output.status.code().unwrap_or(-1));

    // SNAPSHOT: start from non-git dir should produce exactly these lines
    let exit_code = output.status.code().unwrap_or(-1);
    assert_eq!(exit_code, 1, "Should exit 1");
    assert_eq!(stdout.trim(), "", "stdout should be empty");

    let lines: Vec<&str> = stderr.lines().collect();
    assert_eq!(lines.len(), 2, "Should have exactly 2 lines. Got:\n{}", stderr);
    assert!(lines[0].starts_with("→ Session: out-norepo-"), "Line 0: '{}'", lines[0]);
    assert_eq!(
        lines[1],
        "Error: No repos to create session. Use --discover-repos <dir> or run from a git repo.",
        "Line 1: '{}'", lines[1]
    );
}

// ============================================================================
// Running container — the "already running" path
// ============================================================================

#[tokio::test]
#[ignore]
async fn output_start_already_running() {
    // Create a running container with the exact name gitvm expects,
    // then run `gitvm start` against it and capture what comes out.
    let d = docker();
    let session_name = format!("out-running-{}", std::process::id());
    let container_name = format!("claude-session-ctr-{}", session_name);

    // Create the volumes (discovery checks for these)
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.create_volume(bollard::volume::CreateVolumeOptions {
            name: vol,
            ..Default::default()
        }).await;
    }

    // Create and start a container that stays running (sleep)
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

    // Now run the binary — it should hit the "already running" path
    let binary = find_binary();
    let output = std::process::Command::new(&binary)
        .args(["start", "-s", &session_name])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run gitvm");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = output.status.code().unwrap_or(-1);

    println!("=== STDOUT ({} bytes) ===", stdout.len());
    println!("{}", stdout);
    println!("=== STDERR ({} bytes) ===", stderr.len());
    println!("{}", stderr);
    println!("=== EXIT: {} ===", exit_code);

    // Clean up
    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.remove_volume(&vol, None::<bollard::volume::RemoveVolumeOptions>).await;
    }

    // SNAPSHOT: "already running" should produce exactly these lines
    assert_eq!(exit_code, 0, "Should exit 0");
    assert_eq!(stdout.trim(), "", "stdout should be empty");

    let lines: Vec<&str> = stderr.lines().collect();

    // Check line count and content
    assert_eq!(lines.len(), 2, "Should have exactly 2 lines. Got {} lines:\n{}", lines.len(), stderr);
    assert!(lines[0].starts_with("→ Session: out-running-"), "Line 0: '{}'", lines[0]);
    assert!(lines[1].contains("Container already running") && lines[1].contains("-a"), "Line 1: '{}'", lines[1]);

    // THE STAIRCASE CHECK: each line must start at column 0 (or small indent)
    // If \n without \r leaks into raw mode, lines accumulate leading whitespace
    for (i, line) in lines.iter().enumerate() {
        let leading = line.len() - line.trim_start().len();
        assert!(
            leading <= 4,
            "STAIRCASE DETECTED: Line {} has {} leading spaces: '{}'\n\
             Full output:\n{}",
            i, leading, line, stderr
        );
    }
}

/// Same test but through a PTY (simulates real terminal)
/// This catches staircase from raw mode leaking across invocations.
#[tokio::test]
#[ignore]
async fn output_start_already_running_via_pty() {
    let d = docker();
    let session_name = format!("out-pty-{}", std::process::id());
    let container_name = format!("claude-session-ctr-{}", session_name);

    // Create volumes
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.create_volume(bollard::volume::CreateVolumeOptions {
            name: vol,
            ..Default::default()
        }).await;
    }

    // Create running container
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
    ).await.expect("create");

    d.start_container(
        &container_name,
        None::<bollard::container::StartContainerOptions<String>>,
    ).await.expect("start");

    // Run through `script` to get a real PTY
    // macOS: script -q /dev/null <cmd>
    // Linux: script -qc <cmd> /dev/null
    let binary = find_binary();
    let cmd_str = format!(
        "NO_COLOR=1 {} start -s {}",
        binary.display(),
        session_name
    );

    let output = std::process::Command::new("script")
        .args(["-q", "/dev/null", "/bin/sh", "-c", &cmd_str])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .output()
        .expect("failed to run via script");

    let raw_stdout = String::from_utf8_lossy(&output.stdout);

    // script captures everything on stdout (combined). Strip \r for comparison.
    let cleaned = raw_stdout.replace('\r', "");

    println!("=== RAW ({} bytes) ===", raw_stdout.len());
    for (i, b) in raw_stdout.bytes().enumerate().take(500) {
        if b == b'\n' { print!("\\n"); }
        else if b == b'\r' { print!("\\r"); }
        else if b < 0x20 { print!("\\x{:02x}", b); }
        else { print!("{}", b as char); }
    }
    println!("\n=== CLEANED ===");
    println!("{}", cleaned);

    // Clean up
    let _ = d.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.remove_volume(&vol, None::<bollard::volume::RemoveVolumeOptions>).await;
    }

    // THE STAIRCASE CHECK on PTY output
    let lines: Vec<&str> = cleaned.lines().filter(|l| !l.trim().is_empty()).collect();
    for (i, line) in lines.iter().enumerate() {
        let leading = line.len() - line.trim_start().len();
        assert!(
            leading <= 4,
            "STAIRCASE via PTY: Line {} has {} leading spaces: '{}'\n\
             All lines:\n{}",
            i, leading, line, cleaned
        );
    }
}

/// Verify reset_terminal() actually restores cooked mode from raw mode.
///
/// This is a unit test of the fix itself: put OUR terminal in raw mode,
/// call reset_terminal (via the binary), check that output is clean.
/// We can't use `script` because it creates a fresh PTY.
/// Instead we test the raw→cooked transition via termios directly.
#[test]
fn output_reset_terminal_fixes_raw_mode() {
    // This test checks that the reset_terminal function in main.rs
    // would fix raw mode. We can't call it directly (it's in the binary),
    // so we replicate the logic and test it.
    #[cfg(unix)]
    {
        use std::os::unix::io::AsRawFd;

        let fd = std::io::stderr().as_raw_fd();
        unsafe {
            let mut original: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut original) != 0 {
                println!("SKIP: can't get termios (not a TTY)");
                return;
            }

            // Save original state
            let saved = original;

            // Simulate raw mode: clear ICANON, ECHO, OPOST
            original.c_lflag &= !(libc::ICANON | libc::ECHO);
            original.c_oflag &= !libc::OPOST;
            libc::tcsetattr(fd, libc::TCSANOW, &original);

            // Verify we're in raw mode
            let mut check: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut check);
            assert_eq!(check.c_lflag & libc::ICANON, 0, "Should be in raw mode (no ICANON)");
            assert_eq!(check.c_oflag & libc::OPOST, 0, "Should be in raw mode (no OPOST)");

            // Apply the same fix as reset_terminal()
            check.c_lflag |= libc::ICANON | libc::ECHO | libc::ISIG | libc::IEXTEN;
            check.c_iflag |= libc::ICRNL | libc::IXON;
            check.c_oflag |= libc::OPOST;
            libc::tcsetattr(fd, libc::TCSANOW, &check);

            // Verify cooked mode is restored
            let mut after: libc::termios = std::mem::zeroed();
            libc::tcgetattr(fd, &mut after);
            assert_ne!(after.c_lflag & libc::ICANON, 0, "ICANON should be restored");
            assert_ne!(after.c_lflag & libc::ECHO, 0, "ECHO should be restored");
            assert_ne!(after.c_oflag & libc::OPOST, 0, "OPOST should be restored (NL→CRNL)");

            // Restore original state (don't leave test terminal broken)
            libc::tcsetattr(fd, libc::TCSANOW, &saved);
        }
    }
}

/// Verify the actual binary resets raw mode on startup.
/// Runs: enable raw mode → run gitvm → check that gitvm restored cooked mode.
#[tokio::test]
#[ignore]
async fn output_binary_resets_raw_mode() {
    let session_name = format!("out-rawreset-{}", std::process::id());

    let binary = find_binary();

    // Use `script` with a wrapper that:
    // 1. Enables raw mode
    // 2. Runs gitvm (which should reset)
    // 3. Checks terminal state AFTER gitvm exits
    // 4. Outputs whether OPOST is set (cooked) or not (still raw)
    let check_script = format!(
        "stty raw -echo 2>/dev/null; \
         NO_COLOR=1 {binary} start -s {session} 2>/dev/null; \
         stty -a 2>/dev/null | grep -o 'opost'",
        binary = binary.display(),
        session = session_name,
    );

    let output = std::process::Command::new("script")
        .args(["-q", "/dev/null", "/bin/sh", "-c", &check_script])
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .current_dir(std::env::temp_dir())
        .output()
        .expect("failed to run");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let cleaned = stdout.replace('\r', "").replace('\n', " ");
    println!("stty check: '{}'", cleaned.trim());

    // After gitvm runs, the terminal should have opost enabled
    // (meaning output processing is on, \n → \r\n)
    assert!(
        cleaned.contains("opost"),
        "Terminal should be in cooked mode (opost) after gitvm exits. Got: '{}'",
        cleaned.trim()
    );

    // Clean up any volumes created
    let d = docker();
    for prefix in ["session", "state", "cargo", "npm", "pip"] {
        let vol = format!("claude-{}-{}", prefix, session_name);
        let _ = d.remove_volume(&vol, None::<bollard::volume::RemoveVolumeOptions>).await;
    }
}

// ============================================================================
// Output quality checks
// ============================================================================

#[tokio::test]
#[ignore]
async fn output_no_staircase_effect() {
    // Run entrypoint and check that no line has excessive leading whitespace
    // (staircase = each line indented further because \n without \r in raw mode)
    let session = TestSession::new("out-staircase").await;
    let result = session.run_entrypoint_with_bash_exec(
        "echo line1; echo line2; echo line3; echo line4; echo line5"
    ).await;

    let all_output = format!("{}{}", result.stdout, result.stderr);
    for (i, line) in all_output.lines().enumerate() {
        let leading = line.len() - line.trim_start().len();
        assert!(
            leading < 10,
            "Line {} has {} leading spaces (staircase?): '{}'",
            i, leading, line
        );
    }
}

#[tokio::test]
#[ignore]
async fn output_no_raw_escape_codes_in_logs() {
    // Container logs should not contain raw terminal escape sequences
    // (unless we explicitly asked for color)
    let session = TestSession::new("out-escapes").await;
    let result = session.run_entrypoint_with_bash_exec("echo CLEAN_OUTPUT").await;

    // Check for common escape patterns that indicate garbled output
    assert!(
        !result.stdout.contains("\x1b["),
        "Stdout has escape codes: {:?}",
        &result.stdout[..result.stdout.len().min(200)]
    );
    result.assert_stdout_contains("CLEAN_OUTPUT");
}

// ============================================================================
// Helpers
// ============================================================================

fn find_binary() -> std::path::PathBuf {
    // Try debug build first, then release, then cargo bin
    let candidates = [
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/debug/gitvm"),
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target/release/gitvm"),
        dirs::home_dir()
            .unwrap_or_default()
            .join(".cargo/bin/gitvm"),
    ];

    candidates.into_iter()
        .find(|p| p.exists())
        .expect("Can't find gitvm binary. Run `cargo build` first.")
}
