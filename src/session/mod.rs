//! Session discovery, configuration, and lifecycle planning.
//!
//! SessionManager is the read-side of session state:
//!   - Discover what Docker resources exist for a session
//!   - Load/save metadata from disk
//!   - Read config from session volumes
//!   - Scan for repos on the host filesystem
//!   - Plan session creation

use std::path::{Path, PathBuf};

use bollard::container::InspectContainerOptions;
use bollard::Docker;
use futures_util::StreamExt;

use crate::types::{
    Action, ContainerInspect, ContainerName, DiscoveredSession, ImageId, ImageRef, MountInfo,
    MountType, Plan, RepoConfig, SessionConfig, SessionMetadata, SessionName, SessionVolumes,
    VolumeName, VolumeState,
};

/// Manages session discovery, config I/O, and creation planning.
pub struct SessionManager {
    docker: Docker,
    config_dir: PathBuf,   // ~/.config/claude-container
    sessions_dir: PathBuf, // ~/.config/claude-container/sessions
}

/// Plan for creating a new session.
#[derive(Debug)]
pub struct SessionCreatePlan {
    pub name: SessionName,
    pub config: SessionConfig,
    pub volumes_to_create: Vec<VolumeName>,
    pub repos_to_clone: Vec<RepoConfig>,
}

impl Action for SessionCreatePlan {
    type Result = ();
    type Error = crate::types::ContainerError;

    fn preview(self) -> Result<Plan<Self>, Self::Error> {
        let description = format!(
            "Create session '{}': {} volume(s), {} repo(s)",
            self.name,
            self.volumes_to_create.len(),
            self.repos_to_clone.len(),
        );
        Ok(Plan {
            destructive: !self.volumes_to_create.is_empty() || !self.repos_to_clone.is_empty(),
            action: self,
            description,
        })
    }

    fn execute(self) -> Result<Self::Result, Self::Error> {
        // Execution is handled by the lifecycle module, not here.
        // The plan is consumed by the caller who orchestrates Docker calls.
        Ok(())
    }
}

impl SessionManager {
    /// Create a new SessionManager using the default config directory.
    pub fn new(docker: Docker) -> Self {
        let config_dir = dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("~/.config"))
            .join("claude-container");
        let sessions_dir = config_dir.join("sessions");
        Self {
            docker,
            config_dir,
            sessions_dir,
        }
    }

    // ========================================================================
    // Discovery
    // ========================================================================

    /// Discover the current state of a session by inspecting Docker resources.
    ///
    /// Checks volumes and container, returns the appropriate DiscoveredSession variant.
    pub async fn discover(
        &self,
        name: &SessionName,
    ) -> Result<DiscoveredSession, crate::types::ContainerError> {
        let volumes = self.inspect_volumes(name).await?;
        let metadata = self.load_metadata(name);

        // If no volumes exist at all, the session doesn't exist
        if !volumes.session.exists() && !volumes.state.exists() {
            return Ok(DiscoveredSession::DoesNotExist(name.clone()));
        }

        // Check for a container
        let container_name = name.container_name();
        match self.inspect_container(&container_name).await {
            Ok(Some((inspect, running))) => {
                if running {
                    Ok(DiscoveredSession::Running {
                        name: name.clone(),
                        metadata,
                        volumes,
                        container: inspect,
                    })
                } else {
                    Ok(DiscoveredSession::Stopped {
                        name: name.clone(),
                        metadata,
                        volumes,
                        container: inspect,
                    })
                }
            }
            Ok(None) => Ok(DiscoveredSession::VolumesOnly {
                name: name.clone(),
                metadata,
                volumes,
            }),
            Err(e) => Err(e),
        }
    }

    /// Inspect all 5 volumes for a session, returning their existence state.
    async fn inspect_volumes(
        &self,
        name: &SessionName,
    ) -> Result<SessionVolumes, crate::types::ContainerError> {
        let docker = &self.docker;

        let check_volume = |vol_name: VolumeName| async move {
            match docker.inspect_volume(vol_name.as_str()).await {
                Ok(_) => (vol_name, true),
                Err(_) => (vol_name, false),
            }
        };

        let (session_r, state_r, cargo_r, npm_r, pip_r) = tokio::join!(
            check_volume(name.session_volume()),
            check_volume(name.state_volume()),
            check_volume(name.cargo_volume()),
            check_volume(name.npm_volume()),
            check_volume(name.pip_volume()),
        );

        Ok(SessionVolumes {
            session: if session_r.1 {
                VolumeState::Exists {
                    name: session_r.0,
                }
            } else {
                VolumeState::Missing {
                    name: session_r.0,
                }
            },
            state: if state_r.1 {
                VolumeState::Exists {
                    name: state_r.0,
                }
            } else {
                VolumeState::Missing {
                    name: state_r.0,
                }
            },
            cargo: if cargo_r.1 {
                VolumeState::Exists { name: cargo_r.0 }
            } else {
                VolumeState::Missing { name: cargo_r.0 }
            },
            npm: if npm_r.1 {
                VolumeState::Exists { name: npm_r.0 }
            } else {
                VolumeState::Missing { name: npm_r.0 }
            },
            pip: if pip_r.1 {
                VolumeState::Exists { name: pip_r.0 }
            } else {
                VolumeState::Missing { name: pip_r.0 }
            },
        })
    }

    /// Inspect a container, returning (ContainerInspect, is_running) or None if not found.
    async fn inspect_container(
        &self,
        container_name: &ContainerName,
    ) -> Result<Option<(ContainerInspect, bool)>, crate::types::ContainerError> {
        let result = self
            .docker
            .inspect_container(
                container_name.as_str(),
                Some(InspectContainerOptions { size: false }),
            )
            .await;

        match result {
            Ok(info) => {
                let running = info
                    .state
                    .as_ref()
                    .and_then(|s| s.running)
                    .unwrap_or(false);

                let image_name = info
                    .config
                    .as_ref()
                    .and_then(|c| c.image.clone())
                    .unwrap_or_default();

                let image_id = info.image.clone().unwrap_or_default();

                let user = info
                    .config
                    .as_ref()
                    .and_then(|c| c.user.clone())
                    .unwrap_or_default();

                let env_vars = info
                    .config
                    .as_ref()
                    .and_then(|c| c.env.as_ref())
                    .map(|envs| {
                        envs.iter()
                            .filter_map(|e| {
                                let mut parts = e.splitn(2, '=');
                                let key = parts.next()?.to_string();
                                let val = parts.next().unwrap_or("").to_string();
                                Some((key, val))
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let mounts = info
                    .mounts
                    .as_ref()
                    .map(|ms| {
                        ms.iter()
                            .map(|m| {
                                let mount_type =
                                    match m.typ.as_ref().map(|t| format!("{:?}", t)) {
                                        Some(ref s) if s.contains("bind") || s.contains("BIND") => {
                                            MountType::Bind
                                        }
                                        Some(ref s)
                                            if s.contains("volume") || s.contains("VOLUME") =>
                                        {
                                            MountType::Volume
                                        }
                                        Some(ref s)
                                            if s.contains("tmpfs") || s.contains("TMPFS") =>
                                        {
                                            MountType::Tmpfs
                                        }
                                        _ => MountType::Bind,
                                    };
                                MountInfo {
                                    source: PathBuf::from(m.source.as_deref().unwrap_or("")),
                                    destination: PathBuf::from(
                                        m.destination.as_deref().unwrap_or(""),
                                    ),
                                    mount_type,
                                    read_only: m.rw.map(|rw| !rw).unwrap_or(false),
                                }
                            })
                            .collect()
                    })
                    .unwrap_or_default();

                let created = info.created.clone().unwrap_or_default();

                let inspect = ContainerInspect {
                    image_id: ImageId::new(image_id),
                    image_name: ImageRef::new(image_name),
                    user,
                    env_vars,
                    mounts,
                    created,
                };

                Ok(Some((inspect, running)))
            }
            Err(bollard::errors::Error::DockerResponseServerError {
                status_code: 404, ..
            }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ========================================================================
    // Metadata (disk-persisted .env files)
    // ========================================================================

    /// Load session metadata from `~/.config/claude-container/sessions/<name>.env`.
    ///
    /// Returns None if the file doesn't exist or can't be parsed.
    pub fn load_metadata(&self, name: &SessionName) -> Option<SessionMetadata> {
        let path = self.sessions_dir.join(format!("{}.env", name.as_str()));
        let content = std::fs::read_to_string(&path).ok()?;

        let mut meta = SessionMetadata {
            name: name.clone(),
            ..Default::default()
        };

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            if let Some((key, value)) = line.split_once('=') {
                let key = key.trim();
                let value = value.trim().trim_matches('"');
                match key {
                    "DOCKERFILE" => meta.dockerfile = Some(PathBuf::from(value)),
                    "RUN_AS_ROOTISH" => meta.run_as_rootish = parse_bool(value),
                    "RUN_AS_USER" => meta.run_as_user = parse_bool(value),
                    "ENABLE_DOCKER" => meta.enable_docker = parse_bool(value),
                    "EPHEMERAL" => meta.ephemeral = parse_bool(value),
                    "NO_CONFIG" => meta.no_config = parse_bool(value),
                    _ => {} // ignore unknown keys
                }
            }
        }

        Some(meta)
    }

    /// Save session metadata to `~/.config/claude-container/sessions/<name>.env`.
    pub fn save_metadata(
        &self,
        metadata: &SessionMetadata,
    ) -> Result<(), crate::types::ContainerError> {
        std::fs::create_dir_all(&self.sessions_dir)?;

        let path = self
            .sessions_dir
            .join(format!("{}.env", metadata.name.as_str()));

        let mut content = String::new();
        content.push_str("# claude-container session metadata\n");

        if let Some(ref dockerfile) = metadata.dockerfile {
            content.push_str(&format!("DOCKERFILE=\"{}\"\n", dockerfile.display()));
        }
        content.push_str(&format!(
            "RUN_AS_ROOTISH=\"{}\"\n",
            metadata.run_as_rootish
        ));
        content.push_str(&format!("RUN_AS_USER=\"{}\"\n", metadata.run_as_user));
        content.push_str(&format!("ENABLE_DOCKER=\"{}\"\n", metadata.enable_docker));
        content.push_str(&format!("EPHEMERAL=\"{}\"\n", metadata.ephemeral));
        content.push_str(&format!("NO_CONFIG=\"{}\"\n", metadata.no_config));

        std::fs::write(&path, content)?;
        Ok(())
    }

    // ========================================================================
    // Config (from session volume)
    // ========================================================================

    /// Read `.claude-projects.yml` from the session volume.
    ///
    /// Runs a throwaway container to cat the file from the volume.
    pub async fn read_config(
        &self,
        name: &SessionName,
    ) -> Result<Option<SessionConfig>, crate::types::ContainerError> {
        let volume_name = name.session_volume();

        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() % 0xFFFFFF;
        let container_label = format!("cc-cfg-{}-{:x}", name.as_str(), suffix);

        // Remove any leftover container from a previous failed attempt
        let _ = self
            .docker
            .remove_container(
                &container_label,
                Some(bollard::container::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        let config = bollard::container::Config {
            image: Some("alpine:latest".to_string()),
            cmd: Some(vec![
                "cat".to_string(),
                "/session/.claude-projects.yml".to_string(),
            ]),
            host_config: Some(bollard::models::HostConfig {
                binds: Some(vec![format!("{}:/session:ro", volume_name)]),
                ..Default::default()
            }),
            ..Default::default()
        };

        let created = self
            .docker
            .create_container(
                Some(bollard::container::CreateContainerOptions {
                    name: container_label.as_str(),
                    platform: None,
                }),
                config,
            )
            .await?;

        self.docker
            .start_container::<String>(&created.id, None)
            .await?;

        // Wait for exit
        let mut wait_stream = self.docker.wait_container(
            &created.id,
            Some(bollard::container::WaitContainerOptions {
                condition: "not-running",
            }),
        );

        let mut exit_code: i64 = -1;
        while let Some(result) = wait_stream.next().await {
            match result {
                Ok(response) => {
                    exit_code = response.status_code;
                }
                Err(_) => break,
            }
        }

        // Get logs (stdout)
        let mut log_stream = self.docker.logs::<String>(
            &created.id,
            Some(bollard::container::LogsOptions {
                stdout: true,
                stderr: false,
                follow: false,
                ..Default::default()
            }),
        );

        let mut output = String::new();
        while let Some(chunk) = log_stream.next().await {
            if let Ok(log) = chunk {
                output.push_str(&log.to_string());
            }
        }

        // Clean up
        let _ = self
            .docker
            .remove_container(
                &created.id,
                Some(bollard::container::RemoveContainerOptions {
                    force: true,
                    ..Default::default()
                }),
            )
            .await;

        if exit_code != 0 || output.trim().is_empty() {
            return Ok(None);
        }

        let config: SessionConfig = serde_yaml::from_str(&output)?;
        Ok(Some(config))
    }

    // ========================================================================
    // Repo discovery (host filesystem)
    // ========================================================================

    /// Discover git repos in a directory (one level deep).
    ///
    /// Skips worktrees (where `.git` is a file, not a directory).
    pub fn discover_repos(&self, dir: &Path) -> Vec<RepoConfig> {
        let mut repos = Vec::new();

        let entries = match std::fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(_) => return repos,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            let git_dir = path.join(".git");

            // Skip worktrees: .git is a file containing "gitdir: ..."
            if git_dir.is_file() {
                continue;
            }

            if git_dir.is_dir() {
                let name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();

                // Detect current branch
                let branch = git2::Repository::open(&path)
                    .ok()
                    .and_then(|repo| {
                        repo.head().ok().and_then(|head| {
                            head.shorthand().map(|s| s.to_string())
                        })
                    });

                repos.push(RepoConfig {
                    name,
                    host_path: path,
                    extract_enabled: true,
                    branch,
                });
            }
        }

        repos.sort_by(|a, b| a.name.cmp(&b.name));
        repos
    }

    // ========================================================================
    // Main project resolution
    // ========================================================================

    /// Resolve which project is the "main" one for a session.
    ///
    /// Priority: explicit `main: true` > cwd match > session name match > first project.
    pub fn resolve_main_project(
        &self,
        config: &SessionConfig,
        cwd: &Path,
        session_name: &str,
    ) -> Option<String> {
        // 1. Explicit main: true
        for (name, cfg) in &config.projects {
            if cfg.main {
                return Some(name.clone());
            }
        }

        // 2. Match cwd
        for (name, cfg) in &config.projects {
            if cwd == cfg.path || cwd.starts_with(&cfg.path) {
                return Some(name.clone());
            }
        }

        // 3. Match session name
        if config.projects.contains_key(session_name) {
            return Some(session_name.to_string());
        }

        // 4. First project
        config.projects.keys().next().cloned()
    }

    // ========================================================================
    // Plan creation
    // ========================================================================

    /// Plan for creating a new session.
    ///
    /// Inspects existing volumes to determine what needs to be created,
    /// and collects repos that need to be cloned into the session volume.
    pub async fn plan_create(
        &self,
        name: &SessionName,
        config: &SessionConfig,
    ) -> Result<Plan<SessionCreatePlan>, crate::types::ContainerError> {
        let volumes = self.inspect_volumes(name).await?;

        // Determine which volumes need creating
        let volumes_to_create: Vec<VolumeName> = volumes.missing().into_iter().cloned().collect();

        // Collect repos from config
        let repos_to_clone: Vec<RepoConfig> = config
            .projects
            .iter()
            .map(|(proj_name, proj_cfg)| RepoConfig {
                name: proj_name.clone(),
                host_path: proj_cfg.path.clone(),
                extract_enabled: proj_cfg.extract,
                branch: None,
            })
            .collect();

        let description = format!(
            "Create session '{}': {} volume(s) to create, {} repo(s) to clone",
            name,
            volumes_to_create.len(),
            repos_to_clone.len(),
        );

        let plan = SessionCreatePlan {
            name: name.clone(),
            config: config.clone(),
            volumes_to_create,
            repos_to_clone,
        };

        Ok(Plan {
            action: plan,
            description,
            destructive: true,
        })
    }
}

/// Parse a boolean from an env file value.
fn parse_bool(s: &str) -> bool {
    matches!(s.to_lowercase().as_str(), "true" | "1" | "yes")
}
