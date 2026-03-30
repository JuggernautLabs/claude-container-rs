//! Lifecycle module — Docker operations for images, containers, and volumes.
//!
//! Wraps bollard to provide typed, checked Docker interactions.
//! Every function returns domain types from `crate::types`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use bollard::container::{
    Config, CreateContainerOptions, RemoveContainerOptions, StartContainerOptions,
    WaitContainerOptions,
};
use bollard::image::BuildImageOptions;
use bollard::models::{
    ContainerStateStatusEnum, HostConfig, Mount, MountPointTypeEnum, MountTypeEnum,
};
use bollard::volume::CreateVolumeOptions;
use bollard::Docker;
use futures_util::StreamExt;
use sha2::{Digest, Sha256};

use crate::types::{
    ContainerAction, ContainerInspect, ContainerName, ContainerPlan, ImageId, ImageRef,
    ImageValidation, BinaryCheck, MountInfo, MountType, Plan, SessionName, SessionVolumes,
    TokenMount, VolumeName, VolumeState,
};
use crate::types::docker::{ContainerState, DockerState};
use crate::types::error::ContainerError;
use crate::types::image::{CRITICAL_BINARIES, OPTIONAL_BINARIES};

type Result<T> = std::result::Result<T, ContainerError>;

/// How long a validation cache entry is considered fresh (24 hours).
pub const VALIDATION_CACHE_TTL: Duration = Duration::from_secs(24 * 60 * 60);

// ============================================================================
// Container creation arguments
// ============================================================================

/// Arguments for creating a container, beyond image and name.
#[derive(Debug, Clone)]
pub struct ContainerCreateArgs {
    /// Environment variables (KEY=VALUE)
    pub env: Vec<String>,
    /// Bind mounts (host:container or host:container:ro)
    pub binds: Vec<String>,
    /// Named volume mounts
    pub volumes: Vec<(VolumeName, String)>,
    /// Working directory inside the container
    pub working_dir: Option<String>,
    /// Entrypoint override
    pub entrypoint: Option<Vec<String>>,
    /// Command
    pub cmd: Option<Vec<String>>,
    /// User (e.g. "1000:1000" or "root")
    pub user: Option<String>,
    /// Allocate a TTY
    pub tty: bool,
    /// Keep stdin open
    pub open_stdin: bool,
    /// Labels
    pub labels: HashMap<String, String>,
}

impl Default for ContainerCreateArgs {
    fn default() -> Self {
        Self {
            env: Vec::new(),
            binds: Vec::new(),
            volumes: Vec::new(),
            working_dir: None,
            entrypoint: None,
            cmd: None,
            user: None,
            tty: true,
            open_stdin: true,
            labels: HashMap::new(),
        }
    }
}

// ============================================================================
// Container health check result
// ============================================================================

/// Result of checking whether a container is usable as-is or needs work.
#[derive(Debug)]
pub enum ContainerCheck {
    /// Container is running and matches expected state — just attach.
    Ready,
    /// Container is stopped but valid — resume it.
    Resumable,
    /// Container exists but is stale — remove and recreate.
    Stale { reasons: Vec<String> },
    /// No container found — create from scratch.
    Missing,
}

// ============================================================================
// Docker socket discovery (no CLI dependency)
// ============================================================================

/// Discover Docker socket by reading context config files directly.
/// Priority: DOCKER_HOST env → active Docker context → None (bollard defaults).
///
/// Reads ~/.docker/config.json for the active context name, then scans
/// ~/.docker/contexts/meta/<hash>/meta.json to find the socket path.
/// No subprocess calls — pure filesystem reads.
fn discover_docker_host() -> Option<String> {
    // 1. DOCKER_HOST env takes priority
    if let Ok(host) = std::env::var("DOCKER_HOST") {
        if !host.is_empty() {
            return Some(host);
        }
    }

    // 2. Read active Docker context from ~/.docker/config.json
    let home = dirs::home_dir()?;
    let docker_dir = home.join(".docker");
    let config_path = docker_dir.join("config.json");
    let config_str = std::fs::read_to_string(&config_path).ok()?;

    // Extract "currentContext": "..." from config.json
    let ctx_name = config_str
        .split("\"currentContext\"")
        .nth(1)?
        .split('"')
        .nth(1)?;

    if ctx_name == "default" {
        return None; // use bollard default (/var/run/docker.sock)
    }

    // 3. Scan context metadata dirs for matching context name
    let meta_dir = docker_dir.join("contexts").join("meta");
    let entries = std::fs::read_dir(&meta_dir).ok()?;
    for entry in entries.flatten() {
        let meta_path = entry.path().join("meta.json");
        if let Ok(meta_str) = std::fs::read_to_string(&meta_path) {
            let name_match = format!("\"Name\":\"{}\"", ctx_name);
            if meta_str.contains(&name_match) {
                // Extract Host from Endpoints.docker.Host
                if let Some(host) = meta_str
                    .split("\"Host\":\"")
                    .nth(1)
                    .and_then(|s| s.split('"').next())
                {
                    return Some(host.to_string());
                }
            }
        }
    }

    None
}

// ============================================================================
// Lifecycle
// ============================================================================

pub struct Lifecycle {
    docker: Docker,
}

impl Lifecycle {
    /// Connect to the local Docker daemon.
    /// Priority: DOCKER_HOST env → Docker context config files → bollard defaults.
    /// Reads ~/.docker/config.json + context meta.json directly (no CLI dependency).
    ///
    /// Terminates with a clear error if connection fails — no silent fallthrough.
    pub fn new() -> Result<Self> {
        let (docker, _source) = match discover_docker_host() {
            Some(host) => {
                let sock_path = host.strip_prefix("unix://").unwrap_or(&host);
                let d = Docker::connect_with_unix(sock_path, 120, bollard::API_DEFAULT_VERSION)
                    .map_err(|e| ContainerError::DockerUnavailable(format!(
                        "Cannot connect to Docker at {}\n\
                         \n\
                         The socket was discovered from ~/.docker/config.json (context config).\n\
                         Error: {}\n\
                         \n\
                         Check:\n\
                         - Is Docker or Colima running?\n\
                         - Does the socket file exist? (ls -la {})\n\
                         - Try: DOCKER_HOST=unix://<path> gitvm <command>",
                        host, e, sock_path
                    )))?;
                (d, host)
            }
            None => {
                let d = Docker::connect_with_local_defaults()
                    .map_err(|e| ContainerError::DockerUnavailable(format!(
                        "Cannot connect to Docker\n\
                         \n\
                         No DOCKER_HOST set and no active Docker context found in\n\
                         ~/.docker/config.json. Falling back to /var/run/docker.sock.\n\
                         Error: {}\n\
                         \n\
                         Check:\n\
                         - Is Docker or Colima running?\n\
                         - Set DOCKER_HOST=unix://<socket-path> explicitly\n\
                         - Or configure a Docker context: docker context create ...",
                        e
                    )))?;
                (d, "/var/run/docker.sock".to_string())
            }
        };
        Ok(Self { docker })
    }

    /// Get a reference to the Docker client
    pub fn docker_client(&self) -> &Docker {
        &self.docker
    }

    // ========================================================================
    // Docker daemon
    // ========================================================================

    /// Check whether the Docker daemon is reachable.
    pub async fn check_docker(&self) -> DockerState {
        match self.docker.version().await {
            Ok(version) => {
                let ver = version.version.unwrap_or_else(|| "unknown".into());
                DockerState::Available { version: ver }
            }
            Err(e) => {
                let msg = e.to_string();
                if msg.contains("No such file") || msg.contains("connect") {
                    DockerState::NotRunning
                } else {
                    DockerState::NotInstalled
                }
            }
        }
    }

    // ========================================================================
    // Utility image
    // ========================================================================

    /// Ensure the utility image (alpine/git) is available locally.
    /// Pulls it if missing. Cached for the process lifetime — only checks once.
    pub async fn ensure_util_image(&self) {
        use std::sync::atomic::{AtomicBool, Ordering};
        static CHECKED: AtomicBool = AtomicBool::new(false);
        if CHECKED.load(Ordering::Relaxed) { return; }


        let image = "alpine/git";
        if self.docker.inspect_image(image).await.is_ok() {
            CHECKED.store(true, Ordering::Relaxed);
            return;
        }

        eprintln!("  Pulling {}...", image);
        use bollard::image::CreateImageOptions;
        let mut stream = self.docker.create_image(
            Some(CreateImageOptions { from_image: image, ..Default::default() }),
            None, None,
        );
        while let Some(result) = stream.next().await {
            if let Err(e) = result {
                eprintln!("  {} Failed to pull {}: {}", "⚠", image, e);
                return;
            }
        }
        eprintln!("  {} Pulled {}", "✓", image);
        CHECKED.store(true, Ordering::Relaxed);
    }

    // ========================================================================
    // Image operations
    // ========================================================================

    /// Resolve the image ID (sha256 digest) for an image reference.
    pub async fn resolve_image_id(&self, image: &ImageRef) -> Result<ImageId> {
        let inspect = self.docker.inspect_image(image.as_str()).await
            .map_err(|_| ContainerError::ImageNotFound(image.clone()))?;
        Ok(ImageId::new(inspect.id.unwrap_or_else(|| "unknown".into())))
    }

    /// Validate that a Docker image has the required binaries.
    ///
    /// Runs a throwaway container that checks for each binary via `command -v`.
    /// Results are cached by image ID hash at
    /// `~/.config/claude-container/cache/image-validated/<hash>`.
    pub async fn validate_image(&self, image: &ImageRef) -> Result<ImageValidation> {
        // Resolve image ID first
        let inspect = self
            .docker
            .inspect_image(image.as_str())
            .await
            .map_err(|_| ContainerError::ImageNotFound(image.clone()))?;

        let image_id = inspect.id.unwrap_or_default();

        // Check cache
        if let Some(cached) = self.load_validation_cache(&image_id, image) {
            return Ok(cached);
        }

        // Build a check script: for each binary, print "name:ok" or "name:missing"
        let all_binaries: Vec<&str> = CRITICAL_BINARIES
            .iter()
            .chain(OPTIONAL_BINARIES.iter())
            .copied()
            .collect();

        let check_script = all_binaries
            .iter()
            .map(|b| format!("if command -v {b} >/dev/null 2>&1; then echo '{b}:ok'; else echo '{b}:missing'; fi"))
            .collect::<Vec<_>>()
            .join("; ");

        // Create a throwaway container
        let container_name = format!("claude-validate-{}", &hash_string(&image_id)[..12]);

        let mut throwaway_labels = std::collections::HashMap::new();
        throwaway_labels.insert(crate::types::THROWAWAY_LABEL.to_string(), "true".to_string());

        let config = Config {
            image: Some(image.as_str().to_string()),
            entrypoint: Some(vec!["sh".to_string()]),
            cmd: Some(vec!["-c".to_string(), check_script]),
            labels: Some(throwaway_labels),
            ..Default::default()
        };

        // Remove any leftover validation container
        let _ = self
            .docker
            .remove_container(
                &container_name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: container_name.clone(),
                    ..Default::default()
                }),
                config,
            )
            .await?;

        self.docker
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await?;

        // Wait for the container to finish
        let mut wait_stream = self.docker.wait_container(
            &container_name,
            Some(WaitContainerOptions {
                condition: "not-running".to_string(),
            }),
        );

        while let Some(_result) = wait_stream.next().await {
            // Just consume the stream until it ends
        }

        // Get logs
        let log_opts = bollard::container::LogsOptions::<String> {
            stdout: true,
            stderr: true,
            ..Default::default()
        };

        let mut log_stream = self.docker.logs(&container_name, Some(log_opts));
        let mut output = String::new();
        while let Some(result) = log_stream.next().await {
            if let Ok(chunk) = result {
                output.push_str(&chunk.to_string());
            }
        }

        // Clean up
        let _ = self
            .docker
            .remove_container(
                &container_name,
                Some(RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        // Parse results
        let mut results: HashMap<&str, bool> = HashMap::new();
        for line in output.lines() {
            let line = line.trim();
            if let Some((name, status)) = line.split_once(':') {
                results.insert(
                    all_binaries
                        .iter()
                        .find(|b| **b == name)
                        .copied()
                        .unwrap_or(name),
                    status == "ok",
                );
            }
        }

        let critical = CRITICAL_BINARIES
            .iter()
            .map(|name| {
                let present = results.get(name).copied().unwrap_or(false);
                BinaryCheck {
                    name: name.to_string(),
                    present,
                    functional: present, // command -v is sufficient for critical check
                }
            })
            .collect();

        let optional = OPTIONAL_BINARIES
            .iter()
            .map(|name| {
                let present = results.get(name).copied().unwrap_or(false);
                BinaryCheck {
                    name: name.to_string(),
                    present,
                    functional: present,
                }
            })
            .collect();

        let validation = ImageValidation {
            image: image.clone(),
            critical,
            optional,
        };

        // Cache the result
        self.save_validation_cache(&image_id, &validation);

        Ok(validation)
    }

    /// Build a Docker image from a Dockerfile.
    ///
    /// Returns the image ID on success.
    pub async fn build_image(
        &self,
        name: &ImageRef,
        dockerfile: &Path,
        context: &Path,
    ) -> Result<ImageId> {
        // Create a tar archive of the build context
        let tar_bytes = create_build_tar(dockerfile, context)?;

        let dockerfile_relative = dockerfile
            .strip_prefix(context)
            .unwrap_or(dockerfile)
            .to_string_lossy()
            .to_string();

        let options = BuildImageOptions {
            dockerfile: dockerfile_relative,
            t: name.as_str().to_string(),
            rm: true,
            ..Default::default()
        };

        // Pass empty credentials map instead of None. Podman's API rejects
        // the X-Registry-Config header bollard sends when credentials are None
        // (malformed empty JSON). An empty HashMap produces valid JSON "{}".
        let credentials = std::collections::HashMap::new();
        let mut stream = self.docker.build_image(
            options,
            Some(credentials),
            Some(tar_bytes.into()),
        );

        let mut last_id = None;
        let mut step_count = 0u32;
        let mut recent_lines: Vec<String> = Vec::new();
        const TAIL: usize = 6;

        // Single progress bar whose message IS the scrolling log window.
        // indicatif redraws the entire message on each set_message, handling
        // cursor positioning and cleanup automatically.
        let pb = indicatif::ProgressBar::new_spinner();
        pb.set_style(
            indicatif::ProgressStyle::default_spinner()
                .template("  {spinner:.blue} {msg}")
                .unwrap()
                .tick_chars("⠋⠙⠹⠸⠼⠴⠦⠧⠇⠏"),
        );
        pb.enable_steady_tick(std::time::Duration::from_millis(80));
        pb.set_message("Building...");

        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(ref error) = info.error {
                        pb.finish_and_clear();
                        eprintln!("  ✗ Build failed: {}", error);
                        check_build_context_hint(dockerfile, &recent_lines, name);
                        return Err(ContainerError::ImageBuildFailed(error.clone()));
                    }
                    if let Some(ref id) = info.id {
                        last_id = Some(id.clone());
                    }
                    if let Some(ref stream_text) = info.stream {
                        for line in stream_text.lines() {
                            let line = line.trim();
                            if line.is_empty() { continue; }
                            if line.starts_with("Step ") {
                                step_count += 1;
                            }
                            recent_lines.push(line.to_string());
                            pb.set_message(build_log_message(&recent_lines, step_count, TAIL));
                        }
                    }
                    if let Some(ref detail) = info.error_detail {
                        if let Some(ref msg) = detail.message {
                            recent_lines.push(format!("ERROR: {}", msg));
                            pb.set_message(build_log_message(&recent_lines, step_count, TAIL));
                        }
                    }
                }
                Err(e) => {
                    pb.finish_and_clear();
                    let last_step = recent_lines.iter().rev()
                        .find(|l| l.starts_with("Step "))
                        .cloned()
                        .unwrap_or_else(|| "unknown step".to_string());
                    eprintln!("  ✗ Build failed at: {}", last_step);
                    eprintln!("  Error: {}", e);
                    eprintln!();
                    check_build_context_hint(dockerfile, &recent_lines, name);
                    return Err(ContainerError::ImageBuildFailed(format!(
                        "{} — {}", last_step, e
                    )));
                }
            }
        }
        pb.finish_and_clear();
        if step_count > 0 {
            eprintln!("  ✓ Built ({} steps)", step_count);
        }

        // After building, inspect to get the definitive image ID
        let inspect = self.docker.inspect_image(name.as_str()).await?;
        let image_id = inspect.id.unwrap_or_else(|| {
            last_id.unwrap_or_else(|| "unknown".into())
        });

        Ok(ImageId::new(image_id))
    }

    // ========================================================================
    // Container operations
    // ========================================================================

    /// Inspect a container and return its typed state.
    pub async fn inspect_container(&self, name: &ContainerName) -> Result<ContainerState> {
        match self.docker.inspect_container(name.as_str(), None).await {
            Ok(resp) => {
                let info = extract_container_inspect(&resp);
                let is_running = resp
                    .state
                    .as_ref()
                    .and_then(|s| s.status.as_ref())
                    .map(|s| matches!(s, ContainerStateStatusEnum::RUNNING))
                    .unwrap_or(false);

                if is_running {
                    Ok(ContainerState::Running {
                        name: name.clone(),
                        info,
                    })
                } else {
                    Ok(ContainerState::Stopped {
                        name: name.clone(),
                        info,
                    })
                }
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(ContainerState::NotFound {
                name: name.clone(),
            }),
            Err(e) => Err(ContainerError::Docker(e)),
        }
    }

    /// Check whether an existing container is usable or needs to be replaced.
    ///
    /// Compares the container's image and script mounts against expectations.
    pub async fn check_container(
        &self,
        name: &ContainerName,
        expected_image: &ImageRef,
        script_dir: &Path,
    ) -> ContainerCheck {
        let state = match self.inspect_container(name).await {
            Ok(s) => s,
            Err(_) => return ContainerCheck::Missing,
        };

        match state {
            ContainerState::NotFound { .. } => ContainerCheck::Missing,

            ContainerState::Running { ref info, .. } => {
                let problems = check_container_staleness(info, expected_image, script_dir);
                if problems.is_empty() {
                    ContainerCheck::Ready
                } else {
                    ContainerCheck::Stale { reasons: problems }
                }
            }

            ContainerState::Stopped { ref info, .. } => {
                let problems = check_container_staleness(info, expected_image, script_dir);
                if problems.is_empty() {
                    ContainerCheck::Resumable
                } else {
                    ContainerCheck::Stale { reasons: problems }
                }
            }
        }
    }

    /// Create a Docker container.
    pub async fn create_container(
        &self,
        name: &ContainerName,
        image: &ImageRef,
        args: ContainerCreateArgs,
    ) -> Result<()> {
        // Build the list of Mount specs from volumes
        let mut mounts: Vec<Mount> = args
            .volumes
            .iter()
            .map(|(vol_name, target)| Mount {
                target: Some(target.clone()),
                source: Some(vol_name.as_str().to_string()),
                typ: Some(MountTypeEnum::VOLUME),
                read_only: Some(false),
                ..Default::default()
            })
            .collect();

        // Add bind mounts parsed from "host:container[:ro]" strings
        for bind in &args.binds {
            let parts: Vec<&str> = bind.split(':').collect();
            if parts.len() >= 2 {
                let read_only = parts.get(2).map(|s| *s == "ro").unwrap_or(false);
                mounts.push(Mount {
                    target: Some(parts[1].to_string()),
                    source: Some(parts[0].to_string()),
                    typ: Some(MountTypeEnum::BIND),
                    read_only: Some(read_only),
                    ..Default::default()
                });
            }
        }

        let host_config = HostConfig {
            mounts: if mounts.is_empty() {
                None
            } else {
                Some(mounts)
            },
            ..Default::default()
        };

        let config = Config {
            image: Some(image.as_str().to_string()),
            env: if args.env.is_empty() {
                None
            } else {
                Some(args.env.clone())
            },
            working_dir: args.working_dir.clone(),
            entrypoint: args.entrypoint.clone(),
            cmd: args.cmd.clone(),
            user: args.user.clone(),
            tty: Some(args.tty),
            open_stdin: Some(args.open_stdin),
            attach_stdin: Some(true),
            attach_stdout: Some(true),
            attach_stderr: Some(true),
            labels: if args.labels.is_empty() {
                None
            } else {
                Some(args.labels.clone())
            },
            host_config: Some(host_config),
            ..Default::default()
        };

        self.docker
            .create_container(
                Some(CreateContainerOptions {
                    name: name.as_str().to_string(),
                    ..Default::default()
                }),
                config,
            )
            .await?;

        Ok(())
    }

    /// Remove a container (force-kills if running).
    pub async fn remove_container(&self, name: &ContainerName) -> Result<()> {
        self.docker
            .remove_container(
                name.as_str(),
                Some(RemoveContainerOptions {
                    force: true,
                    v: false,
                    ..Default::default()
                }),
            )
            .await?;
        Ok(())
    }

    /// Start a stopped container.
    pub async fn start_container(&self, name: &ContainerName) -> Result<()> {
        self.docker
            .start_container(name.as_str(), None::<StartContainerOptions<String>>)
            .await?;
        Ok(())
    }

    // ========================================================================
    // Volume operations
    // ========================================================================

    /// Check which session volumes exist.
    pub async fn check_volumes(&self, session: &SessionName) -> SessionVolumes {
        let all = session.all_volumes();
        let existing = self.list_existing_volumes(&all).await;

        let exists = |vol: &VolumeName| -> bool {
            existing.iter().any(|e| e == vol.as_str())
        };

        SessionVolumes {
            session: if exists(&all[0]) {
                VolumeState::Exists { name: all[0].clone() }
            } else {
                VolumeState::Missing { name: all[0].clone() }
            },
            state: if exists(&all[1]) {
                VolumeState::Exists { name: all[1].clone() }
            } else {
                VolumeState::Missing { name: all[1].clone() }
            },
            cargo: if exists(&all[2]) {
                VolumeState::Exists { name: all[2].clone() }
            } else {
                VolumeState::Missing { name: all[2].clone() }
            },
            npm: if exists(&all[3]) {
                VolumeState::Exists { name: all[3].clone() }
            } else {
                VolumeState::Missing { name: all[3].clone() }
            },
            pip: if exists(&all[4]) {
                VolumeState::Exists { name: all[4].clone() }
            } else {
                VolumeState::Missing { name: all[4].clone() }
            },
        }
    }

    /// Create all five session volumes (idempotent — skips existing ones).
    pub async fn create_volumes(&self, session: &SessionName) -> Result<()> {
        let volumes = session.all_volumes();
        let existing = self.list_existing_volumes(&volumes).await;

        for vol in &volumes {
            if !existing.iter().any(|e| e == vol.as_str()) {
                self.docker
                    .create_volume(CreateVolumeOptions {
                        name: vol.as_str().to_string(),
                        ..Default::default()
                    })
                    .await?;
            }
        }

        Ok(())
    }

    // ========================================================================
    // Token
    // ========================================================================

    /// Write a token string to a file in the cache directory and return
    /// the mount spec for passing into a container.
    pub fn inject_token(&self, token: &str, cache_dir: &Path) -> Result<TokenMount> {
        let token_dir = cache_dir.join("token");
        std::fs::create_dir_all(&token_dir)?;

        let token_file = token_dir.join("api_key");

        // If the path is a directory (corrupted state), remove it first
        if token_file.is_dir() {
            std::fs::remove_dir_all(&token_file)?;
        }

        std::fs::write(&token_file, token)?;

        // Restrict permissions (best-effort on non-Unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&token_file, perms)?;
        }

        Ok(TokenMount::File {
            host_path: token_file,
            container_path: PathBuf::from("/run/secrets/claude_token"),
        })
    }

    // ========================================================================
    // Plan
    // ========================================================================

    /// Inspect current state and produce a plan for launching a session container.
    ///
    /// This is read-only — it figures out what needs to happen without doing it.
    pub async fn plan_launch(
        &self,
        session: &SessionName,
        image: &ImageRef,
        script_dir: &Path,
    ) -> Result<Plan<ContainerPlan>> {
        let container_name = session.container_name();
        let check = self.check_container(&container_name, image, script_dir).await;
        let volumes = self.check_volumes(session).await;

        let missing_vols: Vec<VolumeName> = volumes
            .missing()
            .into_iter()
            .cloned()
            .collect();

        let (action, description, destructive) = match check {
            ContainerCheck::Ready => (
                ContainerAction::Attach {
                    container: container_name,
                },
                "Container is running — attach".to_string(),
                false,
            ),

            ContainerCheck::Resumable => (
                ContainerAction::Resume {
                    container: container_name,
                },
                "Container is stopped — resume".to_string(),
                false,
            ),

            ContainerCheck::Stale { reasons } => (
                ContainerAction::Rebuild {
                    container: container_name,
                    reasons: reasons.clone(),
                    image: image.clone(),
                },
                format!(
                    "Container is stale — rebuild ({})",
                    reasons.join(", ")
                ),
                true,
            ),

            ContainerCheck::Missing => {
                let vol_names: Vec<VolumeName> = if missing_vols.is_empty() {
                    session.all_volumes().to_vec()
                } else {
                    missing_vols
                };

                (
                    ContainerAction::Create {
                        image: image.clone(),
                        volumes: vol_names,
                    },
                    "No container — create new".to_string(),
                    true,
                )
            }
        };

        Ok(Plan {
            action: ContainerPlan { action },
            description,
            destructive,
        })
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    /// List which of the given volume names actually exist in Docker.
    async fn list_existing_volumes(&self, names: &[VolumeName]) -> Vec<String> {
        let response = self
            .docker
            .list_volumes(None::<bollard::volume::ListVolumesOptions<String>>)
            .await;

        match response {
            Ok(resp) => {
                let docker_volumes: Vec<String> = resp
                    .volumes
                    .unwrap_or_default()
                    .into_iter()
                    .map(|v| v.name)
                    .collect();

                names
                    .iter()
                    .filter(|n| docker_volumes.iter().any(|dv| dv == n.as_str()))
                    .map(|n| n.as_str().to_string())
                    .collect()
            }
            Err(_) => Vec::new(),
        }
    }

    /// Load a cached image validation result.
    ///
    /// Returns `None` if the cache file is missing, older than [`VALIDATION_CACHE_TTL`],
    /// or unparseable.
    fn load_validation_cache(&self, image_id: &str, image: &ImageRef) -> Option<ImageValidation> {
        load_validation_cache_standalone(image_id, image)
    }

    /// Persist an image validation result to cache.
    fn save_validation_cache(&self, image_id: &str, validation: &ImageValidation) {
        let Some(cache_path) = validation_cache_path(image_id) else {
            return;
        };

        if let Some(parent) = cache_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }

        let mut content = String::new();
        for check in &validation.critical {
            let status = if check.present { "ok" } else { "missing" };
            content.push_str(&format!("{}:critical:{}\n", check.name, status));
        }
        for check in &validation.optional {
            let status = if check.present { "ok" } else { "missing" };
            content.push_str(&format!("{}:optional:{}\n", check.name, status));
        }

        let _ = std::fs::write(cache_path, content);
    }
}

// ============================================================================
// Free functions
// ============================================================================

/// Extract our typed ContainerInspect from bollard's response.
fn extract_container_inspect(
    resp: &bollard::models::ContainerInspectResponse,
) -> ContainerInspect {
    let image_id = ImageId::new(resp.image.clone().unwrap_or_default());

    let image_name = resp
        .config
        .as_ref()
        .and_then(|c| c.image.clone())
        .map(ImageRef::new)
        .unwrap_or_else(|| ImageRef::new("unknown"));

    let user = resp
        .config
        .as_ref()
        .and_then(|c| c.user.clone())
        .unwrap_or_default();

    let env_vars = resp
        .config
        .as_ref()
        .and_then(|c| c.env.as_ref())
        .map(|envs| {
            envs.iter()
                .filter_map(|e| {
                    let (k, v) = e.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();

    let mounts = resp
        .mounts
        .as_ref()
        .map(|ms| {
            ms.iter()
                .map(|m| {
                    let mount_type = match m.typ {
                        Some(MountPointTypeEnum::BIND) => MountType::Bind,
                        Some(MountPointTypeEnum::VOLUME) => MountType::Volume,
                        Some(MountPointTypeEnum::TMPFS) => MountType::Tmpfs,
                        _ => MountType::Bind,
                    };
                    MountInfo {
                        source: PathBuf::from(m.source.clone().unwrap_or_default()),
                        destination: PathBuf::from(m.destination.clone().unwrap_or_default()),
                        mount_type,
                        read_only: !m.rw.unwrap_or(true),
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    let created = resp.created.clone().unwrap_or_default();

    ContainerInspect {
        image_id,
        image_name,
        user,
        env_vars,
        mounts,
        created,
    }
}

/// Check whether a container's state is stale relative to expectations.
fn check_container_staleness(
    info: &ContainerInspect,
    expected_image: &ImageRef,
    script_dir: &Path,
) -> Vec<String> {
    let mut reasons = Vec::new();

    // Image mismatch
    if info.image_name.as_str() != expected_image.as_str() {
        reasons.push(format!(
            "image mismatch: have {}, want {}",
            info.image_name, expected_image
        ));
    }

    // Check that cc-entrypoint is bind-mounted from the expected scripts dir
    let entrypoint_mount = info.mounts.iter().find(|m| {
        m.mount_type == MountType::Bind
            && m.destination == Path::new("/usr/local/bin/cc-entrypoint")
    });

    match entrypoint_mount {
        Some(mount) => {
            let expected_prefix = script_dir.to_string_lossy();
            if !mount.source.to_string_lossy().starts_with(expected_prefix.as_ref()) {
                reasons.push(format!(
                    "scripts from {}, expected under {}",
                    mount.source.display(), script_dir.display()
                ));
            }
        }
        None => {
            reasons.push("entrypoint scripts not mounted".to_string());
        }
    }

    // Check user is root (entrypoint needs root for privilege drop)
    if !info.user.is_empty() && info.user != "0" && info.user != "0:0" && info.user != "root" {
        reasons.push(format!("user '{}' (needs root for entrypoint)", info.user));
    }

    reasons
}

/// Hash a string with SHA-256 and return the hex digest.
pub fn hash_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Load a cached image validation result (standalone, no `Lifecycle` needed).
///
/// Checks file modification time against [`VALIDATION_CACHE_TTL`].
/// Returns `None` if the cache is expired, missing, or corrupt.
pub fn load_validation_cache_standalone(image_id: &str, image: &ImageRef) -> Option<ImageValidation> {
    let cache_path = validation_cache_path(image_id)?;

    // Check file mtime against TTL
    let metadata = std::fs::metadata(&cache_path).ok()?;
    let modified = metadata.modified().ok()?;
    let age = SystemTime::now().duration_since(modified).unwrap_or(Duration::MAX);
    if age >= VALIDATION_CACHE_TTL {
        return None;
    }

    let content = std::fs::read_to_string(&cache_path).ok()?;

    // Cache format: one line per binary, "name:critical|optional:ok|missing"
    let mut critical = Vec::new();
    let mut optional = Vec::new();

    for line in content.lines() {
        let parts: Vec<&str> = line.split(':').collect();
        if parts.len() != 3 {
            continue;
        }
        let check = BinaryCheck {
            name: parts[0].to_string(),
            present: parts[2] == "ok",
            functional: parts[2] == "ok",
        };
        match parts[1] {
            "critical" => critical.push(check),
            "optional" => optional.push(check),
            _ => {}
        }
    }

    if critical.is_empty() {
        return None;
    }

    Some(ImageValidation {
        image: image.clone(),
        critical,
        optional,
    })
}

/// Get the cache file path for a validated image ID.
pub fn validation_cache_path(image_id: &str) -> Option<PathBuf> {
    let config_dir = dirs::home_dir()?;
    let hash = hash_string(image_id);
    Some(
        config_dir
            .join(".config/claude-container")
            .join("cache")
            .join("image-validated")
            .join(hash),
    )
}

// ============================================================================
// Build log scrolling display
// ============================================================================

/// Build the progress bar message: header + last N lines of build output.
/// Line width adapts to current terminal size on each call.
fn build_log_message(lines: &[String], step_count: u32, tail: usize) -> String {
    let term_width = crossterm::terminal::size()
        .map(|(cols, _)| cols as usize)
        .unwrap_or(80);
    // 4 chars indent + 2 chars margin
    let line_width = term_width.saturating_sub(6).max(20);

    let header = if step_count > 0 {
        format!("Building (step {})", step_count)
    } else {
        "Building...".to_string()
    };

    let start = lines.len().saturating_sub(tail);
    let mut msg = header;
    for line in &lines[start..] {
        msg.push_str("\n    ");
        msg.push_str(&truncate_line(line, line_width));
    }
    msg
}

/// Check if the failure was due to missing --build-context and print guidance.
fn check_build_context_hint(dockerfile: &Path, lines: &[String], name: &ImageRef) {
    let last_step = lines.iter().rev()
        .find(|l| l.starts_with("Step "))
        .cloned().unwrap_or_default();
    let needs_contexts = last_step.contains("COPY --from=")
        || lines.iter().any(|l| l.contains("COPY failed") && l.contains("--from"));

    if !needs_contexts { return; }

    let build_contexts = parse_build_contexts(dockerfile);
    if build_contexts.is_empty() { return; }

    // Extract the specific name that failed from the last step
    let failed_name = last_step
        .split("COPY --from=").nth(1)
        .and_then(|s| s.split_whitespace().next())
        .unwrap_or("?");

    eprintln!("  Cause: '{}' is not a build stage in this Dockerfile.", failed_name);
    eprintln!("  It's an external build context (a sibling directory) that Docker");
    eprintln!("  needs to be told about with --build-context flags.");
    eprintln!();
    eprintln!("  gitvm doesn't pass --build-context yet, so build manually:");
    eprintln!();
    let ctx_args: String = build_contexts.iter()
        .map(|c| format!("    --build-context {}=../{} \\", c, c))
        .collect::<Vec<_>>()
        .join("\n");
    eprintln!("    cd {} && docker buildx build \\", dockerfile.parent().map(|p| p.display().to_string()).unwrap_or(".".into()));
    eprintln!("{}", ctx_args);
    eprintln!("    -t {} .", name.as_str());
    eprintln!();
    eprintln!("  Then start with the pre-built image:");
    eprintln!("    gitvm start -s <session> --image {}", name.as_str());
}

/// Parse a Dockerfile for COPY --from=<name> references that aren't
/// defined as build stages (FROM ... AS <name>). These require
/// --build-context arguments.
fn parse_build_contexts(dockerfile: &Path) -> Vec<String> {
    let content = match std::fs::read_to_string(dockerfile) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    // Collect stage names (FROM ... AS <name>)
    let stages: Vec<String> = content.lines()
        .filter_map(|line| {
            let line = line.trim().to_uppercase();
            if line.starts_with("FROM ") && line.contains(" AS ") {
                line.split(" AS ").nth(1).map(|s| s.trim().to_lowercase())
            } else {
                None
            }
        })
        .collect();

    // Collect COPY --from=<name> references not in stages
    let mut contexts: Vec<String> = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("COPY --from=") {
            let name = rest.split_whitespace().next().unwrap_or("")
                .trim().to_lowercase();
            if !name.is_empty()
                && !stages.contains(&name)
                && name.parse::<u32>().is_err() // not a numeric stage index
                && !contexts.contains(&name)
            {
                contexts.push(name);
            }
        }
    }
    contexts
}

fn truncate_line(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let truncated: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{}…", truncated)
    }
}

// ============================================================================
// Build context tar creation
// ============================================================================

/// Maximum build context size (200 MB). Beyond this, the build will almost
/// certainly fail or be painfully slow. We error early with guidance.
const MAX_CONTEXT_BYTES: u64 = 200 * 1024 * 1024;

/// Directories always excluded from build context (like .git).
/// These are heavy, never needed in a Docker build, and cause
/// multi-GB context uploads if included.
const ALWAYS_EXCLUDE_DIRS: &[&str] = &[
    ".git",
    "target",         // Rust
    "node_modules",   // JS
    "__pycache__",    // Python
    ".mypy_cache",
    ".pytest_cache",
    "dist",           // various build outputs
    ".next",          // Next.js
    ".nuxt",          // Nuxt.js
    ".tox",           // Python tox
    "vendor",         // Go (when not needed)
];

fn create_build_tar(dockerfile: &Path, context: &Path) -> Result<Vec<u8>> {
    // Load .dockerignore patterns if present
    let dockerignore_patterns = load_dockerignore(context);

    // Estimate context size before building tar
    let (file_count, total_bytes) = estimate_context_size(context, &dockerignore_patterns);
    if total_bytes > MAX_CONTEXT_BYTES {
        let size_mb = total_bytes / (1024 * 1024);
        let has_dockerignore = context.join(".dockerignore").exists();

        let mut msg = format!(
            "Build context is too large ({} MB, {} files)\n\
             \n\
             Context directory: {}\n",
            size_mb, file_count, context.display()
        );

        if !has_dockerignore {
            msg.push_str(&format!(
                "\n\
                 No .dockerignore found. Create one to exclude build artifacts:\n\
                 \n\
                     echo 'target/\\nnode_modules/\\n.git/' > {}/.dockerignore\n\
                 \n\
                 Or build the image manually and use --image:\n\
                 \n\
                     docker build -t <name> -f {} {}\n\
                     gitvm start --session <name> --image <name>\n",
                context.display(), dockerfile.display(), context.display()
            ));
        } else {
            msg.push_str(&format!(
                "\n\
                 .dockerignore exists but the context is still large.\n\
                 Add more exclusions or build manually:\n\
                 \n\
                     docker build -t <name> -f {} {}\n\
                     gitvm start --session <name> --image <name>\n",
                dockerfile.display(), context.display()
            ));
        }

        return Err(ContainerError::ImageBuildFailed(msg));
    }

    let buf: Vec<u8> = Vec::new();
    let mut archive = tar::Builder::new(buf);

    add_dir_to_tar(&mut archive, context, context, &dockerignore_patterns)?;

    // If the Dockerfile is outside the context dir, add it explicitly
    if !dockerfile.starts_with(context) {
        let df_content = std::fs::read(dockerfile)?;
        let mut header = tar::Header::new_gnu();
        header.set_size(df_content.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        archive.append_data(&mut header, "Dockerfile", &df_content[..])?;
    }

    let buf = archive.into_inner().map_err(|e: std::io::Error| e)?;
    Ok(buf)
}

/// Load .dockerignore patterns. Returns empty vec if no file exists.
fn load_dockerignore(context: &Path) -> Vec<String> {
    let path = context.join(".dockerignore");
    match std::fs::read_to_string(&path) {
        Ok(content) => content
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Check if a path should be excluded from build context.
fn is_excluded(relative: &Path, dockerignore: &[String]) -> bool {
    let rel_str = relative.to_string_lossy();

    // Always-excluded directories
    for component in relative.components() {
        let name = component.as_os_str().to_string_lossy();
        if ALWAYS_EXCLUDE_DIRS.iter().any(|&d| d == name.as_ref()) {
            return true;
        }
    }

    // .dockerignore patterns (simple prefix/suffix matching)
    for pattern in dockerignore {
        let pat = pattern.trim_end_matches('/');
        // "target" or "target/" matches directory name at any level
        if relative.components().any(|c| c.as_os_str().to_string_lossy() == pat) {
            return true;
        }
        // "*.log" matches file extension
        if pat.starts_with('*') {
            let suffix = &pat[1..];
            if rel_str.ends_with(suffix) {
                return true;
            }
        }
        // Direct prefix match
        if rel_str.starts_with(pat) {
            return true;
        }
    }

    false
}

/// Estimate total size of build context (respecting exclusions).
fn estimate_context_size(context: &Path, dockerignore: &[String]) -> (u64, u64) {
    let mut file_count = 0u64;
    let mut total_bytes = 0u64;
    estimate_dir_size(context, context, dockerignore, &mut file_count, &mut total_bytes);
    (file_count, total_bytes)
}

fn estimate_dir_size(
    dir: &Path,
    base: &Path,
    dockerignore: &[String],
    file_count: &mut u64,
    total_bytes: &mut u64,
) {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let relative = path.strip_prefix(base).unwrap_or(&path);

        if is_excluded(relative, dockerignore) {
            continue;
        }

        if path.is_dir() {
            estimate_dir_size(&path, base, dockerignore, file_count, total_bytes);
        } else if path.is_file() {
            if let Ok(meta) = path.metadata() {
                *file_count += 1;
                *total_bytes += meta.len();
            }
        }
    }
}

fn add_dir_to_tar(
    builder: &mut tar::Builder<Vec<u8>>,
    dir: &Path,
    base: &Path,
    dockerignore: &[String],
) -> std::io::Result<()> {
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap_or(&path);

            if is_excluded(relative, dockerignore) {
                continue;
            }

            if path.is_dir() {
                add_dir_to_tar(builder, &path, base, dockerignore)?;
            } else if path.is_file() {
                builder.append_path_with_name(&path, relative)?;
            }
        }
    }
    Ok(())
}
