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
// Launch options — flags that affect container creation
// ============================================================================

/// Options that control container creation, wired from CLI flags.
#[derive(Debug, Clone, Default)]
pub struct LaunchOptions {
    /// --continue: resume previous Claude conversation
    pub continue_session: bool,
    /// --prompt: initial prompt for Claude (interactive mode)
    pub initial_prompt: Option<String>,
}

impl LaunchOptions {
    /// Build the environment variables contributed by these options.
    pub fn env_vars(&self) -> Vec<String> {
        let mut env = Vec::new();
        if self.continue_session {
            env.push("CONTINUE_SESSION=1".to_string());
        }
        if let Some(ref prompt) = self.initial_prompt {
            let encoded = base64_encode(prompt);
            env.push(format!("CLAUDE_INITIAL_PROMPT={}", encoded));
        }
        env
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
    opts: &LaunchOptions,
) -> ContainerCreateArgs {
    let mut args = ContainerCreateArgs {
        tty: true,
        open_stdin: true,
        user: Some("0:0".to_string()),
        cmd: Some(vec![
            "/bin/bash".to_string(),
            "-c".to_string(),
            "chmod +x /usr/local/bin/cc-entrypoint /usr/local/bin/cc-developer-setup /usr/local/bin/cc-agent-run 2>/dev/null; if ! command -v bash >/dev/null 2>&1; then echo 'ERROR: bash is required but not found in this image.' >&2; echo '  Install bash in your Dockerfile: RUN apt-get install -y bash' >&2; echo '  Or use the base image: FROM ghcr.io/hypermemetic/claude-container:latest' >&2; exit 1; fi; exec /usr/local/bin/cc-entrypoint".to_string(),
        ]),
        ..Default::default()
    };

    // --- Environment variables ---
    let uid = unsafe { libc::getuid() };
    let gid = unsafe { libc::getgid() };

    args.env.push(format!("HOST_UID={}", uid));
    args.env.push(format!("HOST_GID={}", gid));
    args.env.push(format!("CLAUDE_SESSION_NAME={}", session_name));
    args.env.push("TERM=xterm-256color".to_string());
    args.env.push("PLATFORM=linux".to_string());
    args.env.push("RUN_AS_ROOTISH=1".to_string());

    // Override tool homes from Docker ENV — redirect to developer-owned volumes.
    // Docker ENV CARGO_HOME=/usr/local/cargo is baked into the image and can't be
    // overridden by shell exports in the entrypoint (exec resets env).
    // Setting them here as container env vars takes precedence over image ENV.
    args.env.push("CARGO_HOME=/home/developer/.cargo".to_string());
    args.env.push("CABAL_DIR=/home/developer/.cabal".to_string());
    args.env.push("STACK_ROOT=/home/developer/.stack".to_string());
    args.env.push("NPM_CONFIG_CACHE=/home/developer/.npm".to_string());
    args.env.push("PIP_CACHE_DIR=/home/developer/.cache/pip".to_string());

    // Launch options: --continue, --prompt
    args.env.extend(opts.env_vars());

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

/// Consolidated terminal restore: disable raw mode, show cursor, restore
/// OPOST/ICANON/ECHO. Safe to call multiple times (idempotent).
///
/// This is the ONE function that all exit paths must call to ensure the
/// terminal is left in a usable state. Handles:
/// - crossterm raw mode disable (no-op if not enabled)
/// - Cursor visibility restore (\x1b[?25h)
/// - termios OPOST/ICANON/ECHO/ISIG/IEXTEN/ICRNL/IXON restore
pub fn restore_terminal() {
    // 1. Disable crossterm raw mode (safe even if never enabled)
    let _ = terminal::disable_raw_mode();

    // 2. Show cursor + disable mouse/paste/focus
    // Claude Code enables these for its TUI; we must undo them on detach.
    // NOTE: we do NOT send \x1b[?1049l (leave alternate screen) here because
    // restore_terminal() is called on every startup as a safety measure —
    // leaving the alternate screen on every invocation would cause visible
    // garbage. The alternate screen escape is sent only in detach_from_session().
    print!(concat!(
        "\x1b[?25h",     // show cursor
        "\x1b[?1000l",   // disable mouse click tracking
        "\x1b[?1002l",   // disable mouse drag tracking
        "\x1b[?1003l",   // disable mouse all-motion tracking
        "\x1b[?1006l",   // disable SGR mouse mode
        "\x1b[?2004l",   // disable bracketed paste
        "\x1b[?1004l",   // disable focus events
    ));
    let _ = std::io::Write::flush(&mut std::io::stdout());

    // 3. Restore termios flags
    #[cfg(unix)]
    {
        unsafe {
            let fd = libc::STDERR_FILENO;
            let mut termios: libc::termios = std::mem::zeroed();
            if libc::tcgetattr(fd, &mut termios) != 0 {
                return; // not a terminal
            }

            let mut needs_fix = false;

            if termios.c_oflag & libc::OPOST == 0 {
                termios.c_oflag |= libc::OPOST;
                needs_fix = true;
            }

            if termios.c_lflag & libc::ICANON == 0 {
                termios.c_lflag |= libc::ICANON | libc::ISIG | libc::IEXTEN;
                termios.c_iflag |= libc::ICRNL | libc::IXON;
                needs_fix = true;
            }

            if termios.c_lflag & libc::ECHO == 0 {
                termios.c_lflag |= libc::ECHO;
                needs_fix = true;
            }

            if needs_fix {
                libc::tcsetattr(fd, libc::TCSANOW, &termios);
            }
        }
    }
}

/// RAII guard that restores terminal state on drop.
struct RawModeGuard {
    was_enabled: bool,
}

impl RawModeGuard {
    fn enable() -> Result<Self, ContainerError> {
        // Check if we have a TTY before enabling raw mode
        if !is_tty() {
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
            restore_terminal();
        }
    }
}

/// Check if stdout is a TTY.
pub fn is_tty() -> bool {
    use crossterm::tty::IsTty;
    std::io::stdout().is_tty()
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
/// Uses a SEPARATE container name (claude-reconcile-ctr-{session}) to avoid
/// killing any running session container. Shares the same session volumes.
///
/// Returns Some(description) if reconciliation completed, None if Claude exited without finishing.
pub async fn launch_reconciliation(
    lc: &Lifecycle,
    ready: LaunchReady,
    session_name: &SessionName,
    script_dir: &Path,
    conflict_repos: &[(String, std::path::PathBuf, Vec<String>)],
) -> Result<Option<String>, ContainerError> {
    let session_ctr = session_name.container_name();
    let reconcile_ctr = ContainerName::new(format!("claude-reconcile-ctr-{}", session_name));

    // Check if session container is running — must stop it to avoid
    // two containers writing to the same volume simultaneously
    match lc.inspect_container(&session_ctr).await? {
        crate::types::docker::ContainerState::Running { .. } => {
            let spinner = indicatif::ProgressBar::new_spinner();
            spinner.set_style(indicatif::ProgressStyle::default_spinner()
                .template("  {spinner:.yellow} Stopping session container...")
                .unwrap().tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"));
            spinner.enable_steady_tick(std::time::Duration::from_millis(80));
            let docker = lc.docker_client();
            docker.stop_container(
                session_ctr.as_str(),
                Some(bollard::container::StopContainerOptions { t: 10 }),
            ).await?;
            spinner.finish_and_clear();
            eprintln!("  {} Stopped session container.", colored::Colorize::green("✓"));
        }
        _ => {}
    }

    // Clean up any leftover reconciliation container
    let _ = lc.remove_container(&reconcile_ctr).await;

    // Merge target branch INTO session volume repos — creates real conflict markers
    let engine = crate::sync::SyncEngine::new(lc.docker_client().clone());
    let mut actual_conflicts: Vec<(String, Vec<String>)> = Vec::new();
    for (repo_name, host_path, _trial_files) in conflict_repos.iter() {
        let spinner = indicatif::ProgressBar::new_spinner();
        spinner.set_style(
            indicatif::ProgressStyle::default_spinner()
                .template("  {spinner:.blue} Merging into {msg}...")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        spinner.enable_steady_tick(std::time::Duration::from_millis(80));
        spinner.set_message(repo_name.clone());

        let result = engine.merge_into_volume(session_name, repo_name, host_path, "main").await;
        spinner.finish_and_clear();

        match result {
            Ok(crate::sync::MergeIntoResult::Conflict { files }) => {
                eprintln!("    {} {} — conflict in {} file(s)", colored::Colorize::red("✗"), repo_name, files.len());
                actual_conflicts.push((repo_name.clone(), files));
            }
            Ok(crate::sync::MergeIntoResult::CleanMerge) => {
                eprintln!("    {} {} — merged cleanly", colored::Colorize::green("✓"), repo_name);
            }
            Ok(crate::sync::MergeIntoResult::AlreadyUpToDate) => {
                eprintln!("    {} {} — already up to date", colored::Colorize::dimmed("·"), repo_name);
            }
            Err(e) => {
                eprintln!("    {} {} — {}", colored::Colorize::yellow("⚠"), repo_name, e);
            }
        }
    }

    if actual_conflicts.is_empty() {
        eprintln!("  {} All repos merged cleanly. No conflicts to resolve.", colored::Colorize::green("✓"));
        return Ok(None);
    }

    let mut args = build_create_args(&ready, session_name, script_dir, &LaunchOptions::default());

    // Set agent task
    args.env.push("AGENT_TASK=resolve-conflicts".to_string());

    // Build conflict summary from ACTUAL merge results (not trial merge predictions)
    let mut summary = String::from("I've merged the target branch into your session repos.\n\n");
    summary.push_str("The following repos have merge conflicts that need resolving:\n\n");
    for (repo_name, files) in &actual_conflicts {
        summary.push_str(&format!("## {}\n", repo_name));
        summary.push_str("Conflicted files:\n");
        for f in files {
            summary.push_str(&format!("  - {}\n", f));
        }
        summary.push('\n');
    }
    summary.push_str("Run `git status` in each repo to see the conflict markers (<<<<<<< HEAD).\n");
    summary.push_str("After resolving each file: `git add <file>`\n");
    summary.push_str("After all conflicts resolved: `git commit` then `fin \"<description>\"`\n");

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

    // Create reconciliation container (separate name, same volumes)
    lc.create_container(&reconcile_ctr, &ready.image.image, args).await?;
    lc.start_container(&reconcile_ctr).await?;

    eprintln!("  Launching Claude for conflict resolution...");
    eprintln!();
    use std::io::Write;
    std::io::stderr().flush().ok();

    attach_container(lc, &reconcile_ctr, false).await?;

    // Clean up reconciliation container
    let _ = lc.remove_container(&reconcile_ctr).await;

    // Check for .reconcile-complete marker
    let check = check_reconcile_complete(lc, session_name).await;

    if check.is_some() {
        eprintln!("  {} Reconciliation complete.", colored::Colorize::green("✓"));
        eprintln!("  Run `git-sandbox start -s {}` to resume your session.", session_name);
    } else {
        eprintln!("  {} Exited without completing reconciliation.", colored::Colorize::yellow("⚠"));
        eprintln!("  Session container was stopped. Run `git-sandbox start -s {}` to resume.", session_name);
    }

    Ok(check)
}

/// Check if .reconcile-complete exists in the session volume.
/// Returns Some(description) if reconciliation completed, None otherwise.
async fn check_reconcile_complete(
    lc: &Lifecycle,
    session: &SessionName,
) -> Option<String> {
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
    if create.is_err() { return None; }

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

    if stdout.contains("__NONE__") {
        None
    } else {
        Some(stdout.trim().to_string())
    }
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

    // Clone container name for closures that capture by move
    let ctr_name_for_stdin = container_name.to_string();
    let ctr_name_for_ctrlc = container_name.to_string();

    // Spawn a task to forward host stdin -> container stdin.
    // All input (including Ctrl-C / 0x03) is forwarded directly to the container.
    // The interactive session owns Ctrl-C — we only exit when the container exits.
    // Ctrl-Z (0x1a) detaches from the container without stopping it.
    let eof_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let eof_flag_clone = eof_flag.clone();
    let detach_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let detach_flag_clone = detach_flag.clone();
    let stdin_handle = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => {
                    // EOF / broken pipe
                    eof_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
                Ok(n) => {
                    // Ctrl-Z (0x1a): detach from container (leave it running)
                    if buf[..n].contains(&0x1a) {
                        detach_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                        restore_terminal();
                        eprintln!("\r\n→ Detached from {} (still running). Resume with: git-sandbox session -s <name> start -a",
                            ctr_name_for_stdin);
                        break;
                    }
                    // Forward everything else to the container — including Ctrl-C (0x03).
                    // Claude Code and other interactive programs handle their own signals.
                    if input.write_all(&buf[..n]).await.is_err() {
                        break;
                    }
                    if input.flush().await.is_err() {
                        break;
                    }
                }
                Err(_) => {
                    eof_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                    break;
                }
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

    // During an interactive session, SIGINT should be a no-op on the host side.
    // In raw mode, Ctrl-C arrives as byte 0x03 via stdin (forwarded to the container).
    // SIGINT only fires if raw mode setup is incomplete or on some edge cases —
    // either way, we do NOT want to kill the host process while attached.
    let _ = ctr_name_for_ctrlc; // no longer used in handler
    let ctrlc_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let ctrlc_flag_clone = ctrlc_flag.clone();
    let _ = ctrlc::set_handler(move || {
        // Just set the flag — don't exit. The container owns the session.
        ctrlc_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    // Spawn a container wait task — when the container exits, abort stdin
    // so the attach connection closes cleanly (no "press enter to exit")
    let docker_for_wait = docker.clone();
    let name_for_wait = container_name.as_str().to_string();
    let stdin_handle_clone = stdin_handle.abort_handle();
    let container_exited = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let container_exited_clone = container_exited.clone();
    let _wait_handle = tokio::spawn(async move {
        let mut wait = docker_for_wait.wait_container(
            &name_for_wait,
            Some(bollard::container::WaitContainerOptions { condition: "not-running".to_string() }),
        );
        while let Some(_) = wait.next().await {}
        // Container exited — abort stdin to close the attach connection
        container_exited_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        stdin_handle_clone.abort();
    });

    // Forward container output -> host stdout (main loop, blocks until container exits).
    // We do NOT check ctrlc_flag here — the container owns the session and we keep
    // forwarding output until it actually exits or the stream closes.
    let loop_result = {
        use std::panic::AssertUnwindSafe;
        let handle = tokio::spawn(AssertUnwindSafe(async move {
            let mut stdout = tokio::io::stdout();
            while let Some(result) = output.next().await {
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
        }));
        handle.await
    };

    // Clean up background tasks
    stdin_handle.abort();
    resize_handle.abort();

    // _raw_guard drops here, but we also call restore_terminal() explicitly
    // to cover the panic case (Drop may not run if panic=abort).
    // Leave alternate screen first (only appropriate after interactive session,
    // not in the generic restore_terminal() which runs on every startup).
    print!("\x1b[?1049l");
    let _ = std::io::Write::flush(&mut std::io::stdout());
    restore_terminal();

    // Print appropriate exit message based on how we exited
    if detach_flag.load(std::sync::atomic::Ordering::SeqCst) {
        // Already printed detach message in stdin task
    } else if eof_flag.load(std::sync::atomic::Ordering::SeqCst) {
        eprintln!("\r\n→ Connection lost (container: {}).", container_name);
    }

    // If the output loop panicked, report it but don't re-panic
    // (terminal is already restored above)
    if let Err(join_err) = loop_result {
        if join_err.is_panic() {
            eprintln!("\r\n→ Internal error during attach (panic caught). Terminal restored.");
        }
    }

    Ok(())
}

// ============================================================================
// Interactive exec — run a command with a TTY inside a running container
// ============================================================================

/// Run an interactive command (e.g. bash) inside a running container.
/// Bridges stdin/stdout with raw mode, resize handling, Ctrl-Z detach.
/// The exec session ends when the command exits.
pub async fn exec_interactive(
    lc: &Lifecycle,
    container_name: &ContainerName,
    command: &[String],
    as_root: bool,
) -> Result<(), ContainerError> {
    let docker = lc.docker_client();

    let user = if as_root { "root" } else { "developer" };
    let cmd: Vec<String> = if command.is_empty() {
        vec!["bash".to_string()]
    } else {
        command.to_vec()
    };

    // Create exec with TTY + stdin
    let exec = docker.create_exec(
        container_name.as_str(),
        bollard::exec::CreateExecOptions {
            cmd: Some(cmd),
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            tty: Some(true),
            user: Some(user.to_string()),
            ..Default::default()
        },
    ).await?;

    let exec_id = exec.id.clone();

    // Resize exec TTY to match host terminal
    let (cols, rows) = terminal_size();
    let _ = docker.resize_exec(
        &exec_id,
        bollard::exec::ResizeExecOptions {
            width: cols,
            height: rows,
        },
    ).await;

    // Start exec attached
    let start_result = docker.start_exec(
        &exec_id,
        Some(bollard::exec::StartExecOptions { tty: true, ..Default::default() }),
    ).await?;

    let (mut output, mut input) = match start_result {
        bollard::exec::StartExecResults::Attached { output, input } => (output, input),
        bollard::exec::StartExecResults::Detached => {
            return Ok(()); // shouldn't happen with tty
        }
    };

    // Enable raw mode
    let _raw_guard = RawModeGuard::enable()?;

    // Stdin forwarding — Ctrl-Z detaches, everything else forwarded
    let detach_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let detach_flag_clone = detach_flag.clone();
    let ctr_name = container_name.to_string();
    let stdin_handle = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    if buf[..n].contains(&0x1a) {
                        detach_flag_clone.store(true, std::sync::atomic::Ordering::SeqCst);
                        restore_terminal();
                        eprintln!("\r\n→ Detached from exec on {}.", ctr_name);
                        break;
                    }
                    if input.write_all(&buf[..n]).await.is_err() { break; }
                    if input.flush().await.is_err() { break; }
                }
                Err(_) => break,
            }
        }
    });

    // SIGWINCH resize for exec
    let docker_for_resize = docker.clone();
    let exec_id_for_resize = exec_id.clone();
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
                let _ = docker_for_resize.resize_exec(
                    &exec_id_for_resize,
                    bollard::exec::ResizeExecOptions {
                        width: cols,
                        height: rows,
                    },
                ).await;
            }
        }

        #[cfg(not(unix))]
        {
            let _ = (docker_for_resize, exec_id_for_resize);
            std::future::pending::<()>().await;
        }
    });

    // Suppress SIGINT during interactive session
    let _ = ctrlc::set_handler(move || {});

    // Output forwarding
    {
        let mut stdout = tokio::io::stdout();
        while let Some(result) = output.next().await {
            match result {
                Ok(log) => {
                    let bytes = log.into_bytes();
                    if stdout.write_all(&bytes).await.is_err() { break; }
                    if stdout.flush().await.is_err() { break; }
                }
                Err(_) => break,
            }
        }
    }

    // Cleanup
    stdin_handle.abort();
    resize_handle.abort();
    restore_terminal();

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
    let mut args = build_create_args(&ready, session_name, script_dir, &LaunchOptions::default());

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
            eprintln!("  Rebuilding container...");
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

/// Base64-encode a string using pure Rust (no shell dependency).
fn base64_encode(input: &str) -> String {
    crate::shell_safety::base64_encode(input)
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
    opts: &LaunchOptions,
) -> Result<(), ContainerError> {
    let container_name = session_name.container_name();

    match &ready.container {
        LaunchTarget::Create => {
            eprintln!("  Creating container {}...", container_name);
            let args = build_create_args(&ready, session_name, script_dir, opts);
            lc.create_container(&container_name, &ready.image.image, args).await?;
            lc.start_container(&container_name).await?;
            attach_container(lc, &container_name, false).await?;
        }

        LaunchTarget::Resume(resumable) => {
            eprintln!("  Resuming container {}...", resumable.name);
            lc.start_container(&resumable.name).await?;

            // Race: wait briefly for the container to either settle or exit.
            // If it exits within 1s (crashed entrypoint, bad CMD), report it
            // instead of hanging forever on an empty attach stream.
            let docker = lc.docker_client();
            let wait_fut = async {
                let mut wait = docker.wait_container(
                    resumable.name.as_str(),
                    Some(bollard::container::WaitContainerOptions {
                        condition: "not-running".to_string(),
                    }),
                );
                while let Some(r) = wait.next().await {
                    if let Ok(resp) = r { return Some(resp.status_code); }
                }
                None
            };

            let result = tokio::time::timeout(
                std::time::Duration::from_secs(1),
                wait_fut,
            ).await;

            match result {
                Ok(Some(exit_code)) => {
                    // Container exited within 1 second — it crashed
                    let mut logs = String::new();
                    let mut log_stream = docker.logs(
                        resumable.name.as_str(),
                        Some(bollard::container::LogsOptions::<String> {
                            stdout: true, stderr: true, tail: "20".to_string(),
                            ..Default::default()
                        }),
                    );
                    while let Some(Ok(chunk)) = log_stream.next().await {
                        logs.push_str(&chunk.to_string());
                    }
                    eprintln!("  {} Container exited immediately (code {}).", colored::Colorize::red("✗"), exit_code);
                    if !logs.trim().is_empty() {
                        eprintln!("  Last output:");
                        for line in logs.lines().take(10) {
                            eprintln!("    {}", line);
                        }
                    }
                    eprintln!("  Run `session -s {} rebuild` to create a fresh container.", session_name);
                    return Err(ContainerError::NonInteractive("Container exited immediately".into()));
                }
                _ => {
                    // Container still running after 1s — it's alive, attach
                    // Replay logs from startup so we don't miss entrypoint output
                    attach_container(lc, &resumable.name, true).await?;
                }
            }
        }

        LaunchTarget::Rebuild(confirmed) => {
            eprintln!("  Rebuilding container...");
            // Remove the old container
            lc.remove_container(&container_name).await?;
            // Create fresh
            let args = build_create_args(&ready, session_name, script_dir, opts);
            lc.create_container(&container_name, &ready.image.image, args).await?;
            lc.start_container(&container_name).await?;
            attach_container(lc, &container_name, false).await?;
        }
    }

    // Safety: always restore terminal in case attach didn't clean up
    restore_terminal();
    Ok(())
}
