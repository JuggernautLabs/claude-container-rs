//! Docker subsystem state — images, containers, mounts, runtime

use std::path::PathBuf;
use std::collections::HashMap;
use super::{ContainerName, ImageRef, ImageId, VolumeName, SessionName, THROWAWAY_LABEL, SESSION_LABEL};

// ============================================================================
// Docker daemon
// ============================================================================

/// Is Docker available?
#[derive(Debug)]
pub enum DockerState {
    Available { version: String },
    NotInstalled,
    NotRunning,
}

// ============================================================================
// Image state
// ============================================================================

/// The state of an image we want to use
#[derive(Debug)]
pub enum ImageState {
    /// Image exists and is valid
    Valid {
        reference: ImageRef,
        id: ImageId,
        validation: super::ImageValidation,
    },
    /// Image exists but failed validation
    Invalid {
        reference: ImageRef,
        id: ImageId,
        missing_tools: Vec<String>,
    },
    /// Image needs building from Dockerfile
    NeedsBuild {
        reference: ImageRef,
        dockerfile: PathBuf,
        context: PathBuf,
    },
    /// Image needs rebuilding (Dockerfile newer than image)
    NeedsRebuild {
        reference: ImageRef,
        id: ImageId,
        dockerfile: PathBuf,
        reason: RebuildReason,
    },
    /// Image not found
    Missing {
        reference: ImageRef,
    },
}

#[derive(Debug)]
pub enum RebuildReason {
    DockerfileChanged,
    ExplicitRequest,
}

// ============================================================================
// Container runtime state
// ============================================================================

/// Complete state of a container (everything we can know from inspect)
#[derive(Debug)]
pub enum ContainerState {
    /// Container is actively running
    Running {
        name: ContainerName,
        info: ContainerInspect,
    },
    /// Container exists but is stopped
    Stopped {
        name: ContainerName,
        info: ContainerInspect,
    },
    /// No container with this name
    NotFound {
        name: ContainerName,
    },
}

/// Everything we extract from docker inspect
#[derive(Debug, Clone)]
pub struct ContainerInspect {
    pub image_id: ImageId,
    pub image_name: ImageRef,
    pub user: String,
    pub env_vars: Vec<(String, String)>,
    pub mounts: Vec<MountInfo>,
    pub created: String,
}

/// A single mount point
#[derive(Debug, Clone)]
pub struct MountInfo {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub mount_type: MountType,
    pub read_only: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MountType {
    Bind,
    Volume,
    Tmpfs,
}

/// What's wrong with a mount
#[derive(Debug)]
pub enum MountProblem {
    /// Source is a directory but should be a file (token corruption)
    DirectoryInsteadOfFile { path: PathBuf },
    /// Source file doesn't exist anymore
    SourceMissing { path: PathBuf },
    /// Mount points to wrong script dir (stale entrypoint)
    WrongSource {
        mount: String,
        expected: PathBuf,
        actual: PathBuf,
    },
}

// ============================================================================
// Token state
// ============================================================================

/// How we have (or don't have) an auth token
#[derive(Debug)]
pub enum TokenState {
    /// Token available from environment variable
    FromEnv { token: String },
    /// Token available from config file
    FromFile { path: PathBuf, token: String },
    /// Token available from macOS Keychain
    FromKeychain { token: String },
    /// No token found anywhere
    Missing,
}

/// The injected token mount for a container
#[derive(Debug, Clone)]
pub enum TokenMount {
    /// File mount (normal operation)
    File {
        host_path: PathBuf,
        container_path: PathBuf,
    },
    /// Environment variable (nested container)
    EnvVar {
        var_name: String,
    },
}

// ============================================================================
// Volume safety — type-level enforcement of file ownership
// ============================================================================

/// Who a throwaway container runs as.
/// Required by `ThrowawayConfig` — you can't create a writable container without specifying this.
#[derive(Debug, Clone)]
pub enum RunAs {
    /// The host developer user — files will be accessible in the main container.
    Developer(u32, u32),
    /// Root — only for intentional admin operations (session fix).
    Root,
}

impl RunAs {
    /// Get the host developer's uid:gid.
    pub fn developer() -> Self {
        let uid = unsafe { libc::getuid() };
        let gid = unsafe { libc::getgid() };
        Self::Developer(uid, gid)
    }

    pub fn docker_user(&self) -> String {
        match self {
            Self::Developer(uid, gid) => format!("{}:{}", uid, gid),
            Self::Root => "0:0".to_string(),
        }
    }
}

/// A volume mount with access control.
#[derive(Debug, Clone)]
pub enum VolumeMount {
    /// Read-only — container can't modify files.
    ReadOnly { source: String, target: String },
    /// Writable — container will create/modify files.
    Writable { source: String, target: String },
}

impl VolumeMount {
    pub fn to_bind(&self) -> String {
        match self {
            Self::ReadOnly { source, target } => format!("{}:{}:ro", source, target),
            Self::Writable { source, target } => format!("{}:{}", source, target),
        }
    }
}

/// Build a throwaway container config with volume safety.
/// All throwaway containers MUST use this builder — it enforces:
/// - `RunAs` determines the user (developer or root)
/// - Throwaway label for gc cleanup
/// - Session label for scoped cleanup
pub fn throwaway_config(
    image: &str,
    script: &str,
    mounts: &[VolumeMount],
    run_as: &RunAs,
    session: &SessionName,
) -> bollard::container::Config<String> {
    let binds: Vec<String> = mounts.iter().map(|m| m.to_bind()).collect();

    let mut labels = HashMap::new();
    labels.insert(THROWAWAY_LABEL.to_string(), "true".to_string());
    labels.insert(SESSION_LABEL.to_string(), session.to_string());

    bollard::container::Config {
        image: Some(image.to_string()),
        user: Some(run_as.docker_user()),
        entrypoint: Some(vec!["sh".to_string(), "-c".to_string()]),
        cmd: Some(vec![script.to_string()]),
        labels: Some(labels),
        host_config: Some(bollard::models::HostConfig {
            binds: if binds.is_empty() { None } else { Some(binds) },
            ..Default::default()
        }),
        ..Default::default()
    }
}
