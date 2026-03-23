//! Docker subsystem state — images, containers, mounts, runtime

use std::path::PathBuf;
use super::{ContainerName, ImageRef, ImageId, VolumeName};

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
#[derive(Debug)]
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
