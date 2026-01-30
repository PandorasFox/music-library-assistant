//! Central Database Module
//!
//! Core of MLA's toolkit architecture. Common data layer for all operations.
//!
//! This module is organized into:
//! - `types`: Core data structures (Track, ScanStateEntry, DeploymentStats, Health types)
//! - `queries`: All database operations
//!
//! ## Database Access Patterns
//!
//! - `Database`: Full read-write access, used by mutation/computation workers
//! - `ReadOnlyDb`: Read-only wrapper, used by UI code via `Witch::read_db()`
//!
//! The separation enforces that UI code cannot accidentally write to the database.
//! Variables holding `ReadOnlyDb` should be named `read_db` to make intent clear.

pub mod queries;
pub mod types;

// Re-export core types
pub use queries::{Database, ReadOnlyDb};
pub use types::Track;
// Signal types
pub use types::Signal;
