//! Session type-state machine.
//!
//! A session progresses through states. The type system prevents
//! invalid transitions at compile time:
//!
//!   Uncreated → Created → Running → Stopped → Running (resume)
//!                  ↓                    ↓
//!               Deleted             Deleted
//!
//! You can't `pull` from an Uncreated session.
//! You can't `start` an already Running session.
//! You can't `resume` a session that was never created.

use std::path::PathBuf;
use super::{SessionName, ContainerName, ImageRef, ImageId, VolumeName};

/// Session metadata persisted to disk (~/.config/claude-container/sessions/<name>.env)
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

/// Type-state: session does not exist yet
pub struct Uncreated {
    pub name: SessionName,
}

/// Type-state: session volumes exist, repos cloned
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
    pub container: ContainerInfo,
}

/// Type-state: container exists but is stopped
pub struct Stopped {
    pub name: SessionName,
    pub metadata: SessionMetadata,
    pub volumes: SessionVolumes,
    pub container: ContainerInfo,
}

/// The set of volumes for a session
#[derive(Debug, Clone)]
pub struct SessionVolumes {
    pub session: VolumeName,
    pub state: VolumeName,
    pub cargo: VolumeName,
    pub npm: VolumeName,
    pub pip: VolumeName,
}

impl SessionVolumes {
    pub fn for_session(name: &SessionName) -> Self {
        Self {
            session: name.session_volume(),
            state: name.state_volume(),
            cargo: name.cargo_volume(),
            npm: name.npm_volume(),
            pip: name.pip_volume(),
        }
    }

    pub fn all(&self) -> [&VolumeName; 5] {
        [&self.session, &self.state, &self.cargo, &self.npm, &self.pip]
    }
}

/// Info about a container (from docker inspect)
#[derive(Debug, Clone)]
pub struct ContainerInfo {
    pub name: ContainerName,
    pub image_id: ImageId,
    pub image_name: ImageRef,
    pub user: String,
    pub entrypoint_mount: Option<PathBuf>,
}

/// Transitions — each method consumes self and returns the new state.
/// Invalid transitions don't exist as methods.
impl Uncreated {
    /// Create the session: volumes + clone repos
    pub fn create(self, metadata: SessionMetadata) -> Created {
        let volumes = SessionVolumes::for_session(&self.name);
        Created {
            name: self.name,
            metadata,
            volumes,
        }
    }
}

impl Created {
    /// Start a new container
    pub fn start(self, container: ContainerInfo) -> Running {
        Running {
            name: self.name,
            metadata: self.metadata,
            volumes: self.volumes,
            container,
        }
    }

    /// Delete the session (volumes remain until explicitly purged)
    pub fn delete(self) -> Uncreated {
        Uncreated { name: self.name }
    }
}

impl Running {
    /// Container stops (Claude exits)
    pub fn stop(self) -> Stopped {
        Stopped {
            name: self.name,
            metadata: self.metadata,
            volumes: self.volumes,
            container: self.container,
        }
    }
}

impl Stopped {
    /// Resume the stopped container
    pub fn resume(self) -> Running {
        Running {
            name: self.name,
            metadata: self.metadata,
            volumes: self.volumes,
            container: self.container,
        }
    }

    /// Remove the container (keep volumes) and return to Created state
    pub fn remove_container(self) -> Created {
        Created {
            name: self.name,
            metadata: self.metadata,
            volumes: self.volumes,
        }
    }

    /// Delete everything
    pub fn delete(self) -> Uncreated {
        Uncreated { name: self.name }
    }
}
