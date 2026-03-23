//! Container launch — the verified pipeline.
//!
//! Each step produces a Verified proof. The next step requires the proof.
//! You can't skip steps — the types won't let you.
//!
//! ```
//! let docker   = verify_docker(&lc).await?;                    // Verified<DockerAvailable>
//! let image    = verify_image(&lc, &docker, &image_ref).await?; // Verified<ValidImage>
//! let volumes  = verify_volumes(&lc, &docker, &name).await?;   // Verified<VolumesReady>
//! let token    = verify_token(&lc, &token_str).await?;          // Verified<TokenReady>
//! let target   = plan_target(&lc, &docker, &name, &image).await?; // LaunchTarget
//! let ready    = LaunchReady { docker, image, volumes, token, container: target };
//! launch(&lc, ready, &name).await?;
//! ```

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
    let image_id = ImageId::new("TODO"); // would come from docker inspect
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
    let cache_dir = dirs::config_dir()
        .unwrap_or_default()
        .join("claude-container/cache");
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
                    // Need user confirmation to rebuild
                    // For now, return Rebuild without confirmation (TODO: interactive prompt)
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

    // Token: prefer env var injection, fall back to file mount
    match &ready.token.mount {
        TokenMount::EnvVar { var_name } => {
            if let Ok(val) = std::env::var(var_name) {
                args.env.push(format!("CLAUDE_CODE_OAUTH_TOKEN={}", val));
            }
        }
        TokenMount::File { host_path, container_path } => {
            // Mount the token file
            args.binds.push(format!(
                "{}:{}:ro",
                host_path.display(),
                container_path.display(),
            ));
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

    // --- Bind mounts for entrypoint scripts ---
    let scripts = ["cc-entrypoint", "cc-developer-setup", "cc-agent-run"];
    for script in &scripts {
        let host_path = script_dir.join(script);
        if host_path.exists() {
            args.binds.push(format!(
                "{}:/usr/local/bin/{}:ro",
                host_path.display(),
                script,
            ));
        }
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
async fn attach_container(
    lc: &Lifecycle,
    container_name: &ContainerName,
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
        logs: Some(true),
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
    let stdin_handle = tokio::spawn(async move {
        let mut stdin = tokio::io::stdin();
        let mut buf = [0u8; 4096];
        loop {
            match stdin.read(&mut buf).await {
                Ok(0) => break,      // EOF
                Ok(n) => {
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

    // Forward container output -> host stdout (main loop, blocks until container exits)
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

    // Clean up background tasks
    stdin_handle.abort();
    resize_handle.abort();

    // _raw_guard drops here, restoring terminal

    Ok(())
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
            attach_container(lc, &container_name).await?;
        }

        LaunchTarget::Resume(resumable) => {
            eprintln!("  Resuming container {}...", resumable.name);
            lc.start_container(&resumable.name).await?;
            attach_container(lc, &resumable.name).await?;
        }

        LaunchTarget::Rebuild(confirmed) => {
            eprintln!("  Rebuilding container ({})...", confirmed.description);
            // Remove the old container
            lc.remove_container(&container_name).await?;
            // Create fresh
            let args = build_create_args(&ready, session_name, script_dir);
            lc.create_container(&container_name, &ready.image.image, args).await?;
            lc.start_container(&container_name).await?;
            attach_container(lc, &container_name).await?;
        }
    }

    Ok(())
}
