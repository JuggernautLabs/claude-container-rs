//! Container validation types — stale detection as data, not side effects.

use std::path::PathBuf;
use super::{ContainerName, ImageRef, ImageId};

/// Result of checking a container for staleness
#[derive(Debug)]
pub enum ContainerCheck {
    /// Safe to resume
    Ok,
    /// Needs rebuild — reasons explain why
    Stale(Vec<StaleReason>),
    /// Container doesn't exist
    NotFound,
}

/// Why a container is stale (each variant is a specific, typed reason)
#[derive(Debug)]
pub enum StaleReason {
    /// Image was rebuilt (same name, different ID)
    ImageRebuilt {
        container_image: ImageId,
        current_image: ImageId,
    },
    /// Image name changed entirely
    ImageChanged {
        container_image: ImageRef,
        expected_image: ImageRef,
    },
    /// Entrypoint mount points to wrong script dir
    EntrypointMismatch {
        mounted: PathBuf,
        expected: PathBuf,
    },
    /// Container not running as root (entrypoint needs root)
    WrongUser(String),
    /// Stale env var baked into container
    StaleEnv {
        key: String,
        value: String,
    },
    /// Token mount is corrupted (directory instead of file)
    CorruptedMount(PathBuf),
    /// Critical tool missing from image
    MissingTool(String),
}

impl std::fmt::Display for StaleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ImageRebuilt { container_image, current_image } =>
                write!(f, "image rebuilt ({} → {})", container_image, current_image),
            Self::ImageChanged { container_image, expected_image } =>
                write!(f, "image changed ({} → {})", container_image, expected_image),
            Self::EntrypointMismatch { mounted, expected } =>
                write!(f, "entrypoint: {} → {}", mounted.display(), expected.display()),
            Self::WrongUser(user) =>
                write!(f, "user '{}' (needs root for entrypoint)", user),
            Self::StaleEnv { key, value } =>
                write!(f, "stale env: {}={}", key, value),
            Self::CorruptedMount(path) =>
                write!(f, "corrupted mount: {} is directory", path.display()),
            Self::MissingTool(tool) =>
                write!(f, "image missing: {}", tool),
        }
    }
}

/// What to do about a stale container
#[derive(Debug, PartialEq)]
pub enum StaleAction {
    /// Safe to rebuild (no data at risk)
    Rebuild,
    /// Needs user confirmation (may lose uncommitted container work)
    ConfirmRebuild,
    /// Can't start at all (corrupted mount)
    MustRebuild,
}
