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

        // Fast path: fingerprint match means schema is up-to-date
        if reconciler::fingerprint_matches(&db) {
            return InitialUiState::ProgressScreen;
        }

        // Fingerprint mismatch — compute the actual plan to check
        match reconciler::ReconciliationPlan::compute(db.conn()) {
            Ok(plan) if plan.is_empty() => InitialUiState::ProgressScreen,
            Ok(_) => InitialUiState::SchemaUpdateRequired,
            Err(_) => InitialUiState::SchemaUpdateRequired,
        }
    }
}
