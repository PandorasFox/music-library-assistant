//! Directory browser component.
//!
//! A reusable tree-based directory browser that returns selected paths to the caller.
//! The browser knows nothing about specific operations - it's purely a path selector.

mod input;
mod render;
mod state;
mod types;

pub use state::DirBrowserState;
pub use types::{DirBrowserAction, DirBrowserConfig, DirEntry};
