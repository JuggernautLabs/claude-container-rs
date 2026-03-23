//! Lifecycle module — Docker operations for images, containers, and volumes.
//!
//! Wraps bollard to provide typed, checked Docker interactions.
//! Every function returns domain types from `crate::types`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

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
// Lifecycle
// ============================================================================

pub struct Lifecycle {
    docker: Docker,
}

impl Lifecycle {
    /// Connect to the local Docker daemon.
    /// Auto-detects Colima socket if DOCKER_HOST is not set.
    pub fn new() -> Result<Self> {
        // Auto-detect Colima/Docker Desktop socket if DOCKER_HOST not set
        if std::env::var("DOCKER_HOST").is_err() {
            if let Some(home) = dirs::home_dir() {
                let colima = home.join(".colima/default/docker.sock");
                if colima.exists() {
                    std::env::set_var("DOCKER_HOST", format!("unix://{}", colima.display()));
                }
            }
        }

        let docker = Docker::connect_with_local_defaults()
            .map_err(|e| ContainerError::DockerUnavailable(e.to_string()))?;
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
    // Image operations
    // ========================================================================

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

        let config = Config {
            image: Some(image.as_str().to_string()),
            entrypoint: Some(vec!["sh".to_string()]),
            cmd: Some(vec!["-c".to_string(), check_script]),
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

        let mut stream = self.docker.build_image(
            options,
            None,
            Some(tar_bytes.into()),
        );

        let mut last_id = None;
        while let Some(result) = stream.next().await {
            match result {
                Ok(info) => {
                    if let Some(ref error) = info.error {
                        return Err(ContainerError::DockerUnavailable(format!(
                            "Build failed: {}",
                            error
                        )));
                    }
                    // Track the aux ID (final image ID)
                    if let Some(ref id) = info.id {
                        last_id = Some(id.clone());
                    }
                }
                Err(e) => return Err(ContainerError::Docker(e)),
            }
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
            container_path: PathBuf::from("/home/developer/.claude/api_key"),
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
    fn load_validation_cache(&self, image_id: &str, image: &ImageRef) -> Option<ImageValidation> {
        let cache_path = validation_cache_path(image_id)?;
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

    // Check script directory mount — look for a bind mount whose
    // destination looks like /entrypoint or /scripts
    let script_mount = info.mounts.iter().find(|m| {
        m.mount_type == MountType::Bind
            && (m.destination.to_string_lossy().contains("entrypoint")
                || m.destination.to_string_lossy().contains("scripts"))
    });

    if let Some(mount) = script_mount {
        let expected_source = script_dir.to_string_lossy();
        let actual_source = mount.source.to_string_lossy();
        if actual_source != expected_source {
            reasons.push(format!(
                "script dir mismatch: mounted from {}, expected {}",
                actual_source, expected_source
            ));
        }
    }

    reasons
}

/// Hash a string with SHA-256 and return the hex digest.
fn hash_string(s: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(s.as_bytes());
    hex::encode(hasher.finalize())
}

/// Get the cache file path for a validated image ID.
fn validation_cache_path(image_id: &str) -> Option<PathBuf> {
    let config_dir = dirs::config_dir()?;
    let hash = hash_string(image_id);
    Some(
        config_dir
            .join("claude-container")
            .join("cache")
            .join("image-validated")
            .join(hash),
    )
}

/// Create a tar archive of the build context for `docker build`.
fn create_build_tar(dockerfile: &Path, context: &Path) -> Result<Vec<u8>> {
    let buf: Vec<u8> = Vec::new();
    let mut archive = tar::Builder::new(buf);

    // Walk the context directory and add files
    add_dir_to_tar(&mut archive, context, context)?;

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

fn add_dir_to_tar(
    builder: &mut tar::Builder<Vec<u8>>,
    dir: &Path,
    base: &Path,
) -> std::io::Result<()> {
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let relative = path.strip_prefix(base).unwrap_or(&path);

            // Skip .git directories
            if relative
                .components()
                .any(|c| c.as_os_str() == ".git")
            {
                continue;
            }

            if path.is_dir() {
                add_dir_to_tar(builder, &path, base)?;
            } else if path.is_file() {
                builder.append_path_with_name(&path, relative)?;
            }
        }
    }
    Ok(())
}
