//! Inode Changed Acknowledgement Flow
//!
//! Displays files where the underlying file was replaced (same path, different inode).
//! This is an acknowledgement-only flow - tag differences are handled through the
//! separate OOB tag resolution flow.

pub mod types;
pub mod render;

pub use types::{InodeChangedState, InodeChangedAction};
pub use render::render;
