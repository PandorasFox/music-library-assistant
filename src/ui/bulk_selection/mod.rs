//! Bulk Selection Module
//!
//! Provides centralized selection state management for file listings
//! across resolution flows. Supports multi-select with Ctrl+A toggle
//! and individual selection with Space.
//!
//! Tag editor groupings are excluded from this system as they operate
//! on specific groupings already.

mod state;

pub use state::{BulkSelectionState, SelectionMode};
