//! Core building blocks of `terminale`: PTY sessions, the event bus, and the
//! shared session-state primitives the rest of the workspace consumes.
//!
//! This crate has no UI or rendering dependencies. It is meant to be embeddable
//! by both the graphical front-end and tests/headless tooling.

#![deny(unsafe_op_in_unsafe_fn)]
#![warn(missing_docs)]

pub mod error;
pub mod event;
pub mod session;

pub use error::{CoreError, CoreResult};
pub use event::SessionEvent;
pub use session::{DataNotifier, RemoteResizer, RemoteWriter, Session, SessionId, SpawnSpec};
