//! Central Database Module
//!
//! Core of MM's toolkit architecture. Common data layer for all operations.
//!
//! This module is organized into:
//! - `types`: Core data structures (AudioFile, FileEntry, AudioInfo, Health types)
//! - `queries`: All database operations (Database struct, ReadOnlyDb, query submodules)
//! - `write_thread`: Dedicated DB write thread for eliminating connection contention
//!
//! ## Database Access Patterns
//!
//! - `Database`: Full read-write access, used by mutation/computation workers
//! - `ReadOnlyDb`: Read-only wrapper, used by UI code via `Witch::read_db()`
//!
//! The separation enforces that UI code cannot accidentally write to the database.
//! Variables holding `ReadOnlyDb` should be named `read_db` to make intent clear.

pub mod queries;
mod schema;
pub mod table_schema;
pub mod reconciler;
pub mod data_migrations;
pub mod types;
pub mod write_thread;

// Re-export core types
pub use queries::{Database, ReadOnlyDb};

/// Create a new database with fresh schema. Only for first-time setup.
///
/// `Database::open()` is sealed to `crate::db`; this is the one external escape hatch,
/// requiring proof of first-time-setup context via the token.
pub(crate) fn create_database(
    path: &std::path::Path,
    _token: &crate::ui::startup::first_time_setup::FirstTimeSetupToken,
) -> anyhow::Result<Database> {
    Database::open(path)
}
