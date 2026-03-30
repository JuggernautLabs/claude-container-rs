//! Sync Virtual Machine — typed state transitions over git references.
//!
//! The VM holds the observed state of all repos across three reference points
//! (container, session, target). Operations are typed transitions that check
//! preconditions, dispatch to a backend, and update state from postconditions.
//!
//! Three backends:
//! - `RealBackend`: docker + git2 (production)
//! - `MockBackend`: canned responses (unit tests)
//! - `DryRunBackend`: predicted results (preview)

pub mod state;
pub mod ops;
pub mod backend;
pub mod interpreter;
pub mod git2_ops;
pub mod git2_backend;
pub mod real_backend;
pub mod programs;
pub mod display;

pub use state::*;
pub use ops::*;
pub use backend::*;
pub use interpreter::*;
pub use git2_backend::Git2Backend;
pub use real_backend::RealBackend;
pub use programs::*;
pub use display::render_program;
