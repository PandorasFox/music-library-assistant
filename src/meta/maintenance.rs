//! Database maintenance tasks — the third sibling to Mutations and Computations.
//!
//! These are infrastructure-level, operator-approved DB operations that run
//! before the Eye awakens. They bypass the `accepting_mutations()` gate and
//! the transaction system, but require a `ConfirmationGesture` at the call site.
//!
//! ## Characteristics (shared by all variants)
//!
//! - Run before reasoning reaches Full (pre-observing startup phase)
//! - Need write-capable DB access (not routed through `write_thread::signal_sender()`)
//! - No signal clearing, no post-execution pipeline, no spawned computations
//! - Operator approval required (MigrationApproval view, VacuumPrompt view)

/// A database maintenance task.
///
/// Executed by the Witch via `execute_maintenance()` on the rayon thread pool.
/// Each variant carries its own execution parameters.
#[derive(Debug, Clone)]
pub enum DbMaintenanceTask {
    /// Schema migration: ALTER TABLE, CREATE TABLE, etc.
    ///
    /// Opens its own write-capable DB connection because schema changes cannot
    /// be routed through `write_thread::signal_sender()`.
    Migration {
        /// Target schema version (the version *after* this migration).
        migration_id: u32,
        /// Human-readable description for logging.
        description: String,
    },

    /// VACUUM: reclaim unused pages, defragment the database file.
    ///
    /// Executed via `write_thread::execute_vacuum()` which uses the db_thread's
    /// own write connection (VACUUM requires exclusive access).
    Vacuum,
}

impl DbMaintenanceTask {
    /// Human-readable label for status display and logging.
    pub fn label(&self) -> String {
        match self {
            DbMaintenanceTask::Migration { migration_id, description } => {
                format!("Migration v{}: {}", migration_id, description)
            }
            DbMaintenanceTask::Vacuum => "Database VACUUM".to_string(),
        }
    }
}
