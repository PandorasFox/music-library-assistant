//! Central Database Module
//!
//! Core of MLA's toolkit architecture. Common data layer for all operations.
//!
//! This module is organized into:
//! - `types`: Core data structures (Track, ScanStateEntry, DeploymentStats)
//! - `changes`: Algebraic change tracking types
//! - `decisions`: Conversational decision flow types
//! - `queries`: All database operations

pub mod changes;
pub mod decisions;
pub mod queries;
pub mod types;

// Re-export all public types for backwards compatibility
// Some types are not yet used but are part of the public API
#[allow(unused_imports)]
pub use changes::{ChangeSession, ChangeStatus, ChangeType, PendingChange};
pub use decisions::{Decision, DecisionCategory, DecisionOutcome, DecisionPriority, DecisionStack};
pub use queries::Database;
#[allow(unused_imports)]
pub use types::{DeploymentStats, ScanStateEntry, Track};
