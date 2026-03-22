//! Strongly-typed identifiers — can't accidentally pass an image ID where a container ID goes.

use std::fmt;

/// A session name (e.g. "synapse-cc-ux")
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct SessionName(String);

impl SessionName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Docker volume name for the session workspace
    pub fn session_volume(&self) -> VolumeName {
        VolumeName(format!("claude-session-{}", self.0))
    }

    /// Docker volume name for Claude state (conversation history)
    pub fn state_volume(&self) -> VolumeName {
        VolumeName(format!("claude-state-{}", self.0))
    }

    /// Docker volume name for cargo cache
    pub fn cargo_volume(&self) -> VolumeName {
        VolumeName(format!("claude-cargo-{}", self.0))
    }

    /// Docker volume name for npm cache
    pub fn npm_volume(&self) -> VolumeName {
        VolumeName(format!("claude-npm-{}", self.0))
    }

    /// Docker volume name for pip cache
    pub fn pip_volume(&self) -> VolumeName {
        VolumeName(format!("claude-pip-{}", self.0))
    }

    /// All 5 volume names for this session
    pub fn all_volumes(&self) -> [VolumeName; 5] {
        [
            self.session_volume(),
            self.state_volume(),
            self.cargo_volume(),
            self.npm_volume(),
            self.pip_volume(),
        ]
    }

    /// Docker container name
    pub fn container_name(&self) -> ContainerName {
        ContainerName(format!("claude-session-ctr-{}", self.0))
    }
}

impl fmt::Display for SessionName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Docker volume name (typed, not just a string)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VolumeName(String);

impl VolumeName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for VolumeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Docker container name (typed)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ContainerName(String);

impl ContainerName {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ContainerName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Docker image reference (name:tag or ID)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageRef(String);

impl ImageRef {
    pub fn new(reference: impl Into<String>) -> Self {
        Self(reference.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ImageRef {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// A Docker image ID (sha256:...)
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ImageId(String);

impl ImageId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn short(&self) -> &str {
        if self.0.len() > 19 {
            &self.0[7..19] // skip "sha256:" prefix, take 12 chars
        } else {
            &self.0
        }
    }
}

impl fmt::Display for ImageId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.short())
    }
}

/// A git commit hash
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CommitHash(String);

impl CommitHash {
    pub fn new(hash: impl Into<String>) -> Self {
        Self(hash.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn short(&self) -> &str {
        &self.0[..7.min(self.0.len())]
    }

    /// Validate that this looks like a git SHA (not a ref name)
    pub fn is_valid(&self) -> bool {
        self.0.len() >= 7 && self.0.chars().all(|c| c.is_ascii_hexdigit())
    }
}

impl fmt::Display for CommitHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.short())
    }
}
