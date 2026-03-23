//! Verified wrappers — proof-carrying types.
//!
//! You can't construct a Verified<T> directly. You must go through
//! a validation function that checks the invariant and returns the
//! wrapper. Functions that require the invariant take the wrapper type.
//!
//! ```ignore
//! // Can't call start_container with an unvalidated image
//! fn start_container(image: Verified<ValidImage>, ...) -> ...
//!
//! // Must validate first — this is the only way to get Verified<ValidImage>
//! let image: Verified<ValidImage> = validate_image(image_ref).await?;
//! start_container(image, ...);  // compiles
//! ```ignore

use std::ops::Deref;
use std::fmt;

/// A value that has been verified to meet some invariant.
/// The type parameter `Proof` describes WHAT was verified.
/// You cannot construct this directly — only through verification functions.
#[derive(Clone)]
pub struct Verified<Proof> {
    inner: Proof,
}

impl<P: fmt::Debug> fmt::Debug for Verified<P> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Verified({:?})", self.inner)
    }
}

impl<P> Deref for Verified<P> {
    type Target = P;
    fn deref(&self) -> &P {
        &self.inner
    }
}

impl<P> Verified<P> {
    /// ONLY verification functions should call this.
    /// Not pub — module-level access only.
    pub(crate) fn new_unchecked(proof: P) -> Self {
        Self { inner: proof }
    }

    /// Test-only constructor — allows integration tests to create Verified wrappers.
    /// Mirrors new_unchecked with public visibility for testing.
    #[doc(hidden)]
    pub fn __test_new(proof: P) -> Self {
        Self { inner: proof }
    }

    /// Unwrap the verified value
    pub fn into_inner(self) -> P {
        self.inner
    }
}

// ============================================================================
// Proof types — what was verified
// ============================================================================

/// Proof: Docker daemon is available and responding
#[derive(Debug, Clone)]
pub struct DockerAvailable {
    pub version: String,
}

/// Proof: image has all required binaries (gosu, git, claude, bash)
#[derive(Debug, Clone)]
pub struct ValidImage {
    pub image: super::ImageRef,
    pub image_id: super::ImageId,
    pub validation: super::ImageValidation,
}

/// Proof: session volumes exist
#[derive(Debug, Clone)]
pub struct VolumesReady {
    pub session: super::SessionName,
}

/// Proof: token is available and injectable
#[derive(Debug, Clone)]
pub struct TokenReady {
    pub mount: super::docker::TokenMount,
}

/// Proof: container is safe to resume (passed all staleness checks)
#[derive(Debug, Clone)]
pub struct ContainerResumable {
    pub name: super::ContainerName,
}

/// Proof: container is freshly created and ready to start
#[derive(Debug, Clone)]
pub struct ContainerCreated {
    pub name: super::ContainerName,
}

/// Proof: session config is valid and repos are accessible
#[derive(Debug, Clone)]
pub struct ConfigValid {
    pub config: super::SessionConfig,
}

/// Proof: user confirmed a destructive operation
#[derive(Debug, Clone)]
pub struct UserConfirmed {
    pub description: String,
}

// ============================================================================
// Composite proofs — multiple things verified
// ============================================================================

/// Everything needed to launch a container.
/// You can only construct this by verifying each piece.
#[derive(Debug)]
pub struct LaunchReady {
    pub docker: Verified<DockerAvailable>,
    pub image: Verified<ValidImage>,
    pub volumes: Verified<VolumesReady>,
    pub token: Verified<TokenReady>,
    pub container: LaunchTarget,
}

/// What we're launching into
#[derive(Debug)]
pub enum LaunchTarget {
    /// Create a new container
    Create,
    /// Resume a verified-safe stopped container
    Resume(Verified<ContainerResumable>),
    /// Rebuild: user confirmed removal of stale container
    Rebuild(Verified<UserConfirmed>),
}

/// Everything needed to perform a sync operation
#[derive(Debug)]
pub struct SyncReady {
    pub docker: Verified<DockerAvailable>,
    pub volumes: Verified<VolumesReady>,
    pub config: Verified<ConfigValid>,
}

/// Everything needed to remove a container
#[derive(Debug)]
pub struct RemovalApproved {
    pub container: super::ContainerName,
    pub confirmation: Verified<UserConfirmed>,
}
