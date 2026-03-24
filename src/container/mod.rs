//! Container launch — the verified pipeline.
//!
//! Each step produces a Verified proof. The next step requires the proof.
//! You can't skip steps — the types won't let you.
//!
//! ```ignore
//! let docker   = verify_docker(&lc).await?;                    // Verified<DockerAvailable>
//! let image    = verify_image(&lc, &docker, &image_ref).await?; // Verified<ValidImage>
//! let volumes  = verify_volumes(&lc, &docker, &name).await?;   // Verified<VolumesReady>
//! let token    = verify_token(&lc, &token_str).await?;          // Verified<TokenReady>
//! let target   = plan_target(&lc, &docker, &name, &image).await?; // LaunchTarget
//! let ready    = LaunchReady { docker, image, volumes, token, container: target };
//! launch(&lc, ready, &name).await?;
//! ```ignore

use crate::lifecycle::{ContainerCreateArgs, Lifecycle};
use crate::types::docker::TokenMount;
use crate::types::error::ContainerError;
use crate::types::verified::*;
use crate::types::*;
use std::path::Path;

use bollard::container::{AttachContainerOptions, ResizeContainerTtyOptions};
use crossterm::terminal;
use futures_util::StreamExt;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Step 1: Verify Docker is available
pub async fn verify_docker(lc: &Lifecycle) -> Result<Verified<DockerAvailable>, ContainerError> {
    match lc.check_docker().await {
        docker::DockerState::Available { version } => {
            Ok(Verified::new_unchecked(DockerAvailable { version }))
        }
        docker::DockerState::NotRunning => {
            Err(ContainerError::DockerUnavailable("Docker daemon not running".into()))
        }
        docker::DockerState::NotInstalled => {
            Err(ContainerError::DockerUnavailable("Docker not installed".into()))
        }
    }
}

/// Step 2: Verify image meets the container protocol
pub async fn verify_image(
    lc: &Lifecycle,
    _docker: &Verified<DockerAvailable>,  // proof that docker is up
    image: &ImageRef,
) -> Result<Verified<ValidImage>, ContainerError> {
    let validation = lc.validate_image(image).await?;
    if !validation.is_valid() {
        let missing = validation.missing_critical().iter().map(|s| s.to_string()).collect();
        return Err(ContainerError::ImageInvalid {
            image: image.clone(),
            missing,
        });
    }
    let image_id = lc.resolve_image_id(image).await?;
    Ok(Verified::new_unchecked(ValidImage {
        image: image.clone(),
        image_id,
        validation,
    }))
}

/// Step 3: Verify session volumes exist (create if needed)
pub async fn verify_volumes(
    lc: &Lifecycle,
    _docker: &Verified<DockerAvailable>,
    name: &SessionName,
) -> Result<Verified<VolumesReady>, ContainerError> {
    lc.create_volumes(name).await?;
    Ok(Verified::new_unchecked(VolumesReady {
        session: name.clone(),
    }))
}

/// Step 4: Verify token is available
pub fn verify_token(
    lc: &Lifecycle,
    token: &str,
) -> Result<Verified<TokenReady>, ContainerError> {
    let cache_dir = dirs::home_dir()
        .unwrap_or_default()
        .join(".config/claude-container/cache");
    let mount = lc.inject_token(token, &cache_dir)?;
    Ok(Verified::new_unchecked(TokenReady { mount }))
}

/// Step 5: Determine launch target (requires docker + image verified)
pub async fn plan_target(
    lc: &Lifecycle,
    _docker: &Verified<DockerAvailable>,
    name: &SessionName,
    image: &Verified<ValidImage>,
    script_dir: &Path,
) -> Result<LaunchTarget, ContainerError> {
    let container_name = name.container_name();

    match lc.inspect_container(&container_name).await? {
        docker::ContainerState::NotFound { .. } => {
            Ok(LaunchTarget::Create)
        }
        docker::ContainerState::Running { .. } => {
            // Can't create — already running
            // Caller decides: attach or error
            Err(ContainerError::ContainerRunning(container_name))
        }
        docker::ContainerState::Stopped { info, .. } => {
            let check = lc.check_container(&container_name, &image.image, script_dir).await;
            match check {
                crate::lifecycle::ContainerCheck::Ready | crate::lifecycle::ContainerCheck::Resumable => {
                    Ok(LaunchTarget::Resume(Verified::new_unchecked(ContainerResumable {
                        name: container_name,
                    })))
                }
                crate::lifecycle::ContainerCheck::Stale { reasons } => {
                    // Return Rebuild — caller prompts for confirmation
                    Ok(LaunchTarget::Rebuild(Verified::new_unchecked(UserConfirmed {
                        description: format!("Rebuild container: {}", reasons.join(", ")),
                    })))
                }
                crate::lifecycle::ContainerCheck::Missing => {
                    Ok(LaunchTarget::Create)
                }
            }
        }
    }
}

// ============================================================================
// Container creation arguments builder
// ============================================================================

/// Build the ContainerCreateArgs for a new session container.
fn build_create_args(
    ready: &LaunchReady,
    session_name: &SessionName,
    script_dir: &Path,
) -> ContainerCreateArgs {
    let mut args = ContainerCreateArgs {
        tty: true,
        open_stdin: true,
        user: Some("0:0".to_string()),
        cmd: Some(vec![
            "/bin/bash".to_string(),
            "-c".to_string(),
            "chmod +x /usr/local/bin/cc-entrypoint /usr/local/bin/cc-developer-setup /usr/local/bin/cc-agent-run 2>/dev/null; exec /usr/local/bin/cc-entrypoint".to_string(),
        ]),
        ..Default::default()
    };

    // --- Environment variables ---
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    args.env.push(format!("HOST_UID={}", uid));
    args.env.push(format!("HOST_GID={}", gid));
    args.env.push(format!("CLAUDE_SESSION_NAME={}", session_name));
    args.env.push(format!("TERM={}", std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into())));
    args.env.push("PLATFORM=linux".to_string());
    args.env.push("RUN_AS_ROOTISH=1".to_string());

    // Token: always use env var — file mounts to /run/secrets/ fail on Colima/Docker Desktop
    // because the directory doesn't exist in the image and Docker creates it as a dir.
    // The entrypoint checks CLAUDE_CODE_OAUTH_TOKEN_NESTED first.
    match &ready.token.mount {
        TokenMount::File { host_path, .. } => {
            // Read the token from the file and pass as env var
            if let Ok(token_content) = std::fs::read_to_string(host_path) {
                args.env.push(format!("CLAUDE_CODE_OAUTH_TOKEN_NESTED={}", token_content.trim()));
            }
        }
        TokenMount::EnvVar { var_name } => {
            if let Ok(val) = std::env::var(var_name) {
                args.env.push(format!("CLAUDE_CODE_OAUTH_TOKEN_NESTED={}", val));
            }
        }
    }

    // --- Volume mounts ---
    // Session workspace
    args.volumes.push((
        session_name.session_volume(),
        "/workspace".to_string(),
    ));
    // Claude state
    args.volumes.push((
        session_name.state_volume(),
        "/home/developer/.claude".to_string(),
    ));
    // Cargo cache
    args.volumes.push((
        session_name.cargo_volume(),
        "/home/developer/.cargo".to_string(),
    ));
    // npm cache
    args.volumes.push((
        session_name.npm_volume(),
        "/home/developer/.npm".to_string(),
    ));
    // pip cache
    args.volumes.push((
        session_name.pip_volume(),
        "/home/developer/.cache/pip".to_string(),
    ));

    // --- Bind mounts for entrypoint scripts (embedded, materialized to script_dir) ---
    for script in ["cc-entrypoint", "cc-developer-setup", "cc-agent-run"] {
        args.binds.push(format!(
            "{}:/usr/local/bin/{}:ro",
            script_dir.join(script).display(),
            script,
        ));
    }

    // --- SSH and gitconfig (read-only, best-effort) ---
    if let Some(home) = dirs::home_dir() {
        let ssh_dir = home.join(".ssh");
        if ssh_dir.is_dir() {
            args.binds.push(format!(
                "{}:/home/developer/.ssh:ro",
                ssh_dir.display(),
            ));
        }
        let gitconfig = home.join(".gitconfig");
        if gitconfig.is_file() {
            args.binds.push(format!(
                "{}:/home/developer/.gitconfig:ro",
                gitconfig.display(),
            ));
        }
    }

    // --- Labels ---
    args.labels.insert("claude-container.session".to_string(), session_name.to_string());
    args.labels.insert("claude-container.managed".to_string(), "true".to_string());

    args
}

// ============================================================================
// Terminal management
// ============================================================================

/// RAII guard that restores terminal state on drop.
struct RawModeGuard {
    was_enabled: bool,
}

impl RawModeGuard {
    fn enable() -> Result<Self, ContainerError> {
        // Check if we have a TTY before enabling raw mode
        if !atty_stdout() {
            return Ok(Self { was_enabled: false });
        }
        terminal::enable_raw_mode()
            .map_err(|e| ContainerError::Io(std::io::Error::new(std::io::ErrorKind::Other, e)))?;
        Ok(Self { was_enabled: true })
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        if self.was_enabled {
            let _ = terminal::disable_raw_mode();
        }
    }
}

/// Check if stdout is a TTY (without pulling in a full crate).
fn atty_stdout() -> bool {
    #[cfg(unix)]
    {
        unsafe { libc::isatty(libc::STDOUT_FILENO) != 0 }
    }
    #[cfg(not(unix))]
    {
        false
    }
}

/// Get the current terminal size, or a sensible default.
fn terminal_size() -> (u16, u16) {
    terminal::size().unwrap_or((80, 24))
}

// ============================================================================
// Attach — bridge host stdin/stdout to container
// ============================================================================

/// Attach to a running container, bridging stdin/stdout/stderr.
///
/// This function takes ownership of the terminal (raw mode) and blocks until
/// the container exits or the connection drops. Terminal state is restored
/// on return (including on panic, via Drop guard).
/// Launch a reconciliation container: Claude resolves merge conflicts interactively.
///
/// The container is created/rebuilt with:
///   - AGENT_TASK=resolve-conflicts
///   - Host repos bind-mounted read-only at /host/<repo>
///   - Conflict summary injected via AGENT_CONTEXT
///   - Claude sees CLAUDE.md with conflict details
///   - User interacts until Claude calls `fin "description"`
///
/// Returns true if reconciliation completed (.reconcile-complete exists).
pub async fn launch_reconciliation(
    lc: &Lifecycle,
    ready: LaunchReady,
    session_name: &SessionName,
    script_dir: &Path,
    conflict_repos: &[(String, std::path::PathBuf, Vec<String>)], // (name, host_path, conflict_files)
) -> Result<bool, ContainerError> {
    let container_name = session_name.container_name();

    // Remove existing container — reconciliation always gets a fresh one
    let _ = lc.remove_container(&container_name).await;

    let mut args = build_create_args(&ready, session_name, script_dir);

    // Set agent task
    args.env.push("AGENT_TASK=resolve-conflicts".to_string());

    // Build conflict summary
    let mut summary = String::from("Merge conflicts detected during pull:\n\n");
    for (repo_name, _, files) in conflict_repos {
        summary.push_str(&format!("## {}\n", repo_name));
        for f in files {
            summary.push_str(&format!("  - {}\n", f));
        }
        summary.push('\n');
    }
    summary.push_str("Resolve all conflicts, commit, then run: fin \"<description>\"\n");

    // Base64-encode the context for AGENT_CONTEXT env var
    let context_b64 = base64_encode(&summary);
    args.env.push(format!("AGENT_CONTEXT={}", context_b64));

    // Bind-mount host repos read-only at /host/<repo_name>
    for (repo_name, host_path, _) in conflict_repos {
        args.binds.push(format!(
            "{}:/host/{}:ro",
            host_path.display(),
            repo_name,
        ));
    }

    // Create, start, attach
    lc.create_container(&container_name, &ready.image.image, args).await?;
    lc.start_container(&container_name).await?;

    eprintln!("  Launching Claude for conflict resolution...");
    eprintln!();
    use std::io::Write;
    std::io::stderr().flush().ok();

    attach_container(lc, &container_name, false).await?;

    // After Claude exits, check for .reconcile-complete marker
    let check = check_reconcile_complete(lc, session_name).await;
    Ok(check)
}

/// Check if .reconcile-complete exists in the session volume
async fn check_reconcile_complete(
    lc: &Lifecycle,
    session: &SessionName,
) -> bool {
    let volume = session.session_volume();
    let container_name = format!("cc-check-reconcile-{}", session);
    let docker = lc.docker_client();

    let _ = docker.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    let config = bollard::container::Config {
        image: Some("alpine/git".to_string()),
        entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
        cmd: Some(vec!["test -f /session/.reconcile-complete && cat /session/.reconcile-complete || echo __NONE__".to_string()]),
        host_config: Some(bollard::models::HostConfig {
            binds: Some(vec![format!("{}:/session:ro", volume)]),
            ..Default::default()
        }),
        ..Default::default()
    };

    let create = docker.create_container(
        Some(bollard::container::CreateContainerOptions { name: container_name.as_str(), platform: None }),
        config,
    ).await;
    if create.is_err() { return false; }

    let _ = docker.start_container(
        &container_name,
        None::<bollard::container::StartContainerOptions<String>>,
    ).await;

    let mut wait = docker.wait_container(
        &container_name,
        None::<bollard::container::WaitContainerOptions<String>>,
    );
    while let Some(_) = wait.next().await {}

    let mut stdout = String::new();
    let mut logs = docker.logs(
        &container_name,
        Some(bollard::container::LogsOptions::<String> {
            stdout: true,
            follow: false,
            ..Default::default()
        }),
    );
    while let Some(Ok(chunk)) = logs.next().await {
        stdout.push_str(&chunk.to_string());
    }

    let _ = docker.remove_container(
        &container_name,
        Some(bollard::container::RemoveContainerOptions { force: true, ..Default::default() }),
    ).await;

    !stdout.contains("__NONE__")
}

/// Public entry point: attach to an already-running container.
pub async fn attach_to_running(
    lc: &Lifecycle,
    container_name: &ContainerName,
    replay_logs: bool,
) -> Result<(), ContainerError> {
    attach_container(lc, container_name, replay_logs).await
}

async fn attach_container(
    lc: &Lifecycle,
    container_name: &ContainerName,
    replay_logs: bool,
) -> Result<(), ContainerError> {
    let docker = lc.docker_client();

    // Resize container TTY to match host terminal
    let (cols, rows) = terminal_size();
    let _ = docker.resize_container_tty(
        container_name.as_str(),
        ResizeContainerTtyOptions {
            width: cols,
            height: rows,
        },
    ).await;

    // Attach to container streams
    let attach_opts = AttachContainerOptions::<String> {
        stdin: Some(true),
        stdout: Some(true),
        stderr: Some(true),
        stream: Some(true),
        logs: Some(replay_logs),
        ..Default::default()
    };

    let attach = docker
        .attach_container(container_name.as_str(), Some(attach_opts))
        .await?;

    let mut output = attach.output;
    let mut input = attach.input;

    // Enable raw mode so keystrokes go straight to the container
    let _raw_guard = RawModeGuard::enable()?;

    // Spawn a task to forward host stdin -> container stdin
    // Detect Ctrl-C (0x03) in raw mode and exit
    let ctrlc_count = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
    let ctrlc_count_clone = ctrlc_count.clone();
    let stdin_handle = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,      // EOF
                Ok(n) => {
                    // Check for Ctrl-C (0x03) — in raw mode it comes as a byte, not SIGINT
                    if buf[..n].contains(&0x03) {
                        let count = ctrlc_count_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        if count >= 1 {
                            // Second Ctrl-C: force exit
                            let _ = crossterm::terminal::disable_raw_mode();
                            eprint!("\x1b[?25h"); // show cursor
                            eprintln!("\r\n→ Detached.");
                            std::process::exit(0);
                        }
                        // First Ctrl-C: forward to container (Claude handles it)
                    }
                    if input.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    if input.flush().await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // Spawn a task to handle terminal resize (SIGWINCH)
    let docker_for_resize = docker.clone();
    let name_for_resize = container_name.as_str().to_string();
    let resize_handle = tokio::spawn(async move {
        #[cfg(unix)]
        {
            use signal_hook::consts::SIGWINCH;
            use signal_hook_tokio::Signals;

            let mut signals = match Signals::new([SIGWINCH]) {
                Ok(s) => s,
                Err(_) => return,
            };

            while let Some(_sig) = signals.next().await {
                let (cols, rows) = terminal::size().unwrap_or((80, 24));
                let _ = docker_for_resize.resize_container_tty(
                    &name_for_resize,
                    ResizeContainerTtyOptions {
                        width: cols,
                        height: rows,
                    },
                ).await;
            }
        }

        #[cfg(not(unix))]
        {
            // No SIGWINCH on non-unix; just keep the task alive
            let _ = (docker_for_resize, name_for_resize);
            std::future::pending::<()>().await;
        }
    });

    // Set up Ctrl-C handler that restores terminal before exiting
    let ctrlc_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctrlc_flag_clone = ctrlc_flag.clone();
    let _ = ctrlc::set_handler(move || {
        ctrlc_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        // Restore terminal immediately (raw mode guard is in another scope)
        let _ = crossterm::terminal::disable_raw_mode();
        eprint!("\x1b[?25h"); // show cursor
        eprintln!("\n\r→ Detached from container.");
        std::process::exit(0);
    });

    // Forward container output -> host stdout (main loop, blocks until container exits)
    let mut stdout = tokio::io::stdout();
    while let Some(result) = output.next().await {
        if ctrlc_flag.load(std::sync::atomic::Ordering::SeqCst) {
            break;
        }
        match result {
            Ok(log) => {
                let bytes = log.into_bytes();
                if stdout.write_all(&bytes).await.is_err() {
                    break;
                }
                if stdout.flush().await.is_err() {
                    break;
                }
            }
            Err(_) => break,
        }
    }

    // Clean up background tasks
    stdin_handle.abort();
    resize_handle.abort();

    // _raw_guard drops here, restoring terminal

    // Restore cursor visibility — Claude Code hides it for its TUI
    print!("\x1b[?25h");
    let _ = std::io::Write::flush(&mut std::io::stdout());

    Ok(())
}

// ============================================================================
// Headless run — non-interactive prompt execution
// ============================================================================

/// Run a prompt headlessly: create/start container, wait for exit, collect output.
///
/// Unlike `launch()`, this does NOT attach a terminal. Instead it:
/// 1. Creates the container with AGENT_TASK=run and the prompt as AGENT_PROMPT
/// 2. Starts the container (no TTY attach)
/// 3. Waits for the container to exit
/// 4. Collects stdout/stderr logs
/// 5. Returns the captured output
pub async fn run_headless(
    lc: &Lifecycle,
    ready: LaunchReady,
    session_name: &SessionName,
    script_dir: &Path,
    prompt: &str,
) -> Result<String, ContainerError> {
    let container_name = session_name.container_name();

    // Build args, then inject run-mode overrides
    let mut args = build_create_args(&ready, session_name, script_dir);

    // Set AGENT_TASK=run so cc-developer-setup uses -p (print) mode
    args.env.push("AGENT_TASK=run".to_string());

    // Pass the prompt via env var (base64-encoded to avoid quoting issues)
    let prompt_b64 = base64_encode(prompt);
    args.env.push(format!("AGENT_PROMPT={}", prompt_b64));

    // Headless: no TTY, no stdin
    args.tty = false;
    args.open_stdin = false;

    match &ready.container {
        LaunchTarget::Create => {
            eprintln!("  Creating container {}...", container_name);
            lc.create_container(&container_name, &ready.image.image, args).await?;
        }
        LaunchTarget::Resume(resumable) => {
            eprintln!("  Resuming container {}...", resumable.name);
            // For resume, we just start the existing container — can't inject new args.
            // Remove and recreate so we get the run-mode env vars.
            lc.remove_container(&resumable.name).await?;
            lc.create_container(&container_name, &ready.image.image, args).await?;
        }
        LaunchTarget::Rebuild(confirmed) => {
            eprintln!("  Rebuilding container ({})...", confirmed.description);
            lc.remove_container(&container_name).await?;
            lc.create_container(&container_name, &ready.image.image, args).await?;
        }
    }

    // Start container
    eprintln!("  Starting headless run...");
    lc.start_container(&container_name).await?;

    // Wait for container to exit
    let docker = lc.docker_client();
    let mut wait_stream = docker.wait_container(
        container_name.as_str(),
        Some(bollard::container::WaitContainerOptions {
            condition: "not-running".to_string(),
        }),
    );

    let mut exit_code: i64 = -1;
    while let Some(result) = wait_stream.next().await {
        match result {
            Ok(response) => {
                exit_code = response.status_code;
            }
            Err(e) => {
                // Stream error — container may have been removed
                eprintln!("  Warning: wait stream error: {}", e);
                break;
            }
        }
    }

    eprintln!("  Container exited with code {}", exit_code);

    // Collect logs
    let log_opts = bollard::container::LogsOptions::<String> {
        stdout: true,
        stderr: true,
        ..Default::default()
    };

    let mut log_stream = docker.logs(container_name.as_str(), Some(log_opts));
    let mut output = String::new();
    while let Some(result) = log_stream.next().await {
        if let Ok(chunk) = result {
            output.push_str(&chunk.to_string());
        }
    }

    Ok(output)
}

/// Base64-encode a string (simple implementation, no external crate needed).
fn base64_encode(input: &str) -> String {
    use std::process::Command;
    // Use the system base64 command since we don't have a base64 crate
    let output = Command::new("base64")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child.stdin.take().unwrap().write_all(input.as_bytes())?;
            child.wait_with_output()
        });

    match output {
        Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_string(),
        Err(_) => {
            // Fallback: just pass raw (the entrypoint will handle it)
            input.to_string()
        }
    }
}

// ============================================================================
// Launch — the main entry point
// ============================================================================

/// Final step: launch the container. Requires ALL verifications passed.
/// This is the ONLY function that can create/start a container.
///
/// Takes ownership of `ready` (all proofs consumed), plus a reference to the
/// Lifecycle for Docker API calls and the session name for building args.
pub async fn launch(
    lc: &Lifecycle,
    ready: LaunchReady,
    session_name: &SessionName,
    script_dir: &Path,
) -> Result<(), ContainerError> {
    let container_name = session_name.container_name();

    match &ready.container {
        LaunchTarget::Create => {
            eprintln!("  Creating container {}...", container_name);
            let args = build_create_args(&ready, session_name, script_dir);
            lc.create_container(&container_name, &ready.image.image, args).await?;
            lc.start_container(&container_name).await?;
            attach_container(lc, &container_name, false).await?;
        }

        LaunchTarget::Resume(resumable) => {
            eprintln!("  Resuming container {}...", resumable.name);
            lc.start_container(&resumable.name).await?;
            attach_container(lc, &resumable.name, false).await?;
        }

        LaunchTarget::Rebuild(confirmed) => {
            eprintln!("  Rebuilding container ({})...", confirmed.description);
            // Remove the old container
            lc.remove_container(&container_name).await?;
            // Create fresh
            let args = build_create_args(&ready, session_name, script_dir);
            lc.create_container(&container_name, &ready.image.image, args).await?;
            lc.start_container(&container_name).await?;
            attach_container(lc, &container_name, false).await?;
        }
    }

    Ok(())
}
