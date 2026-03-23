//! Session type-state machine.
//!
//!   Uncreated → Created → Running → Stopped → Running (resume)
//!                  ↓                    ↓
//!               Deleted             Deleted
//!
//! Invalid transitions are compile errors.

use std::path::PathBuf;
use super::ids::{SessionName, ImageRef};
use super::volume::SessionVolumes;
use super::docker::ContainerInspect;

/// Session metadata persisted to disk
#[derive(Debug, Clone)]
pub struct SessionMetadata {
    pub name: SessionName,
    pub dockerfile: Option<PathBuf>,
    pub run_as_rootish: bool,
    pub run_as_user: bool,
    pub enable_docker: bool,
    pub ephemeral: bool,
    pub no_config: bool,
}

impl Default for SessionMetadata {
    fn default() -> Self {
        Self {
            name: SessionName::new(""),
            dockerfile: None,
            run_as_rootish: true,
            run_as_user: false,
            enable_docker: false,
            ephemeral: false,
            no_config: false,
        }
    }
}

/// Type-state: session does not exist
pub struct Uncreated {
    pub name: SessionName,
}

/// Type-state: volumes exist, repos cloned, no container
pub struct Created {
    pub name: SessionName,
    pub metadata: SessionMetadata,
    pub volumes: SessionVolumes,
}

/// Type-state: container is running
pub struct Running {
    pub name: SessionName,
    pub metadata: SessionMetadata,
    pub volumes: SessionVolumes,
    pub container: ContainerInspect,
}

/// Type-state: container exists but stopped
pub struct Stopped {
    pub name: SessionName,
    pub metadata: SessionMetadata,
    pub volumes: SessionVolumes,
    pub container: ContainerInspect,
}

// --- Transitions (consume self, return new state) ---

impl Uncreated {
    pub fn create(self, metadata: SessionMetadata, volumes: SessionVolumes) -> Created {
        Created { name: self.name, metadata, volumes }
    }
}

impl Created {
    pub fn start(self, container: ContainerInspect) -> Running {
        Running { name: self.name, metadata: self.metadata, volumes: self.volumes, container }
    }
    pub fn delete(self) -> Uncreated {
        Uncreated { name: self.name }
    }
}

impl Running {
    pub fn stop(self) -> Stopped {
        Stopped { name: self.name, metadata: self.metadata, volumes: self.volumes, container: self.container }
    }
}

impl Stopped {
    pub fn resume(self) -> Running {
        Running { name: self.name, metadata: self.metadata, volumes: self.volumes, container: self.container }
    }
    pub fn remove_container(self) -> Created {
        Created { name: self.name, metadata: self.metadata, volumes: self.volumes }
    }
    pub fn delete(self) -> Uncreated {
        Uncreated { name: self.name }
    }
}

// --- Runtime discovery (we don't know the state at compile time) ---

/// Discovered session state — what we find when we look at Docker
#[derive(Debug)]
pub enum DiscoveredSession {
    /// No volumes, no container
    DoesNotExist(SessionName),
    /// Volumes exist, no container
    VolumesOnly {
        name: SessionName,
        metadata: Option<SessionMetadata>,
        volumes: SessionVolumes,
    },
    /// Stopped container
    Stopped {
        name: SessionName,
        metadata: Option<SessionMetadata>,
        volumes: SessionVolumes,
        container: ContainerInspect,
    },
    /// Running container
    Running {
        name: SessionName,
        metadata: Option<SessionMetadata>,
        volumes: SessionVolumes,
        container: ContainerInspect,
    },
}
