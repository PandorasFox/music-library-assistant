//! UI-Witch startup coordination.
//!
//! The Witch checks database state at startup and determines what the UI
//! should show first. The UI is responsible for the details (querying
//! migration descriptions, rendering modals, etc.).
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::path::Path;

use crate::corpus::db::Database;
use crate::corpus::mutations::MigrationRegistry;

// ============================================================================
// Initial UI State
// ============================================================================

/// Initial state the Witch determines at startup.
///
/// This is a discriminant only - the UI queries details as needed.
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
    ///
    /// The Witch checks whether migrations are needed; the UI handles details.
    pub fn determine(db_path: &Path) -> Self {
        // No database file → first-time setup
        if !db_path.exists() {
            return InitialUiState::FirstTimeSetup;
        }

        // Try to open and check for migrations
        let db = match Database::open(db_path) {
            Ok(db) => db,
            Err(_) => {
                // Can't open database - treat as first-time setup
                // (the setup flow will handle the actual error)
                return InitialUiState::FirstTimeSetup;
            }
        };

        // Check for pending migrations
        let registry = MigrationRegistry::new();
        if registry.needs_migration(&db) {
            return InitialUiState::MigrationRequired;
        }

        // No setup needed - normal startup
        InitialUiState::ProgressScreen
    }
}
