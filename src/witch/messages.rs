//! UI-Witch startup coordination.
//!
//! The Witch checks database state at startup and determines what the UI
//! should show first.

use std::path::Path;

use crate::db::{Database, reconciler};

/// Initial state the Witch determines at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialUiState {
    /// First-time setup - no database exists.
    FirstTimeSetup,
    /// Schema update required — reconciliation plan is non-empty.
    SchemaUpdateRequired,
    /// Normal startup - proceed directly to progress screen.
    ProgressScreen,
}

impl InitialUiState {
    /// Determine initial UI state based on database state.
    pub fn determine(db_path: &Path) -> Self {
        if !db_path.exists() {
            return InitialUiState::FirstTimeSetup;
        }

        let db = match Database::open_read_only(db_path) {
            Ok(db) => db,
            Err(_) => return InitialUiState::FirstTimeSetup,
        };

        // Check schema changes + data migrations together
        let schema_dirty = !reconciler::fingerprint_matches(&db);
        let has_data_migrations = reconciler::has_pending_data_migrations(&db);

        if !schema_dirty && !has_data_migrations {
            return InitialUiState::ProgressScreen;
        }

        if schema_dirty {
            // Schema fingerprint mismatch — compute actual plan to verify
            match reconciler::ReconciliationPlan::compute(db.conn()) {
                Ok(plan) if plan.is_empty() && !has_data_migrations => {
                    InitialUiState::ProgressScreen
                }
                _ => InitialUiState::SchemaUpdateRequired,
            }
        } else {
            // Schema is fine but data migrations are pending
            InitialUiState::SchemaUpdateRequired
        }
    }
}
