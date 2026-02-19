//! UI-Witch startup coordination.
//!
//! The Witch checks database state at startup and determines what the UI
//! should show first.

use std::path::Path;

use crate::db::{Database, ReadOnlyDb};
use crate::meta::mutations::MigrationRegistry;

/// Initial state the Witch determines at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitialUiState {
    /// First-time setup - no database exists.
    FirstTimeSetup,
    /// Migrations are pending and require user approval.
    MigrationRequired,
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
        let read_db = ReadOnlyDb::new(&db);

        let registry = MigrationRegistry::new();
        if registry.needs_migration(&read_db) {
            return InitialUiState::MigrationRequired;
        }

        InitialUiState::ProgressScreen
    }
}
