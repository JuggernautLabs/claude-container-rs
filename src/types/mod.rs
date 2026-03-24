//! Core types — strong typing for the entire container protocol.
//!
//! Every subsystem's state is represented as enums/structs.
//! The type system prevents invalid states and transitions at compile time.

pub mod ids;
pub mod session;
pub mod container;
pub mod image;
pub mod volume;
pub mod repo;
pub mod config;
pub mod snapshot;
pub mod action;
pub mod verified;
pub mod git;
pub mod docker;
pub mod agent;
pub mod error;

pub use ids::*;
pub use session::*;
pub use container::*;
pub use image::*;
pub use volume::*;
pub use repo::*;
pub use config::*;
pub use snapshot::*;
pub use action::*;
pub use verified::*;
pub use git::*;
pub use docker::*;
pub use agent::*;
pub use error::*;

/// Label key used to mark throwaway containers for garbage collection.
pub const THROWAWAY_LABEL: &str = "claude-container.throwaway";

/// Label key used to associate a throwaway container with a session.
pub const SESSION_LABEL: &str = "claude-container.session";
