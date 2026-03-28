//! Startup maintenance, schema reconciliation, and auto-indexing.
//!
//! Handles database schema checks and updates, vacuum management,
//! and automatic indexing of unindexed corpus files discovered by derivation.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use crate::config;
use crate::db::Database;
use crate::meta::mutations::Mutation;

impl super::Witch {
    // -------------------------------------------------------------------------
    // Auto-Indexing
    // -------------------------------------------------------------------------

    /// Request an async check for unindexed corpus files.
    ///
    /// Offloads the DB query to a blocking task. When results arrive via the
    /// offload channel, the handler either queues index mutations or triggers
    /// content analysis (depending on `source`).
    pub(super) fn request_auto_index_check(
        &self,
        source: super::types::AutoIndexSource,
    ) {
        let tx = self.offload_tx.clone();
        tokio::task::spawn_blocking(move || {
            let mutations = auto_index_query_blocking();
            let _ = tx.send(super::types::OffloadResult::AutoIndexResult { mutations, source });
        });
    }

    // -------------------------------------------------------------------------
    // Migration-Aware Startup Methods
    // -------------------------------------------------------------------------

    /// Queue schema reconciliation for async execution.
    ///
    /// Called internally by the Witch during startup when schema update is needed.
    pub(super) fn queue_schema_reconciliation(&mut self) {
        use crate::meta::maintenance::DbMaintenanceTask;

        crate::logging::log_general("[WITCH] Queueing schema reconciliation");
        self.queue_maintenance(DbMaintenanceTask::SchemaReconciliation);
    }

    /// Queue a VACUUM for async execution.
    ///
    /// Called internally by the Witch during startup when vacuum threshold is exceeded.
    pub(super) fn queue_vacuum(&mut self) {
        use crate::meta::maintenance::DbMaintenanceTask;

        self.queue_maintenance(DbMaintenanceTask::Vacuum);
    }

    /// Request an async vacuum check. Result arrives via offload channel.
    pub(super) fn request_vacuum_check(&mut self) {
        self.vacuum_check_pending = true;
        let threshold = self.vacuum_threshold;
        let tx = self.offload_tx.clone();
        tokio::task::spawn_blocking(move || {
            let needed = check_vacuum_needed_blocking(threshold);
            let _ = tx.send(super::types::OffloadResult::VacuumCheck { needed });
        });
    }
}

// ============================================================================
// Free functions for blocking offloaded work
// ============================================================================

/// Query for unindexed corpus files (blocking, off main thread).
///
/// Opens its own read-only DB connection, queries UnindexedFileSignal rows,
/// and builds IndexFileFromPath mutations for each.
fn auto_index_query_blocking() -> Vec<Mutation> {
    let db_path = match config::get_db_path() {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };
    let db = match Database::open_read_only(&db_path) {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let read_db = crate::db::ReadOnlyDb::new(&db);

    let unindexed = match read_db.get_unindexed_signals_for::<crate::zones::CorpusZone>() {
        Ok(files) => files,
        Err(e) => {
            crate::logging::log_error(format!(
                "[AUTO-INDEX] Failed to query unindexed signals: {}",
                e
            ));
            return Vec::new();
        }
    };

    if unindexed.is_empty() {
        return Vec::new();
    }

    let resolver = crate::corpus::paths::get_resolver();
    unindexed
        .iter()
        .map(|(_inode, path)| {
            let abs_path = resolver.resolve_for_zone(
                crate::db::types::Zone::Corpus,
                std::path::Path::new(path),
            );
            Mutation::IndexFileFromPath(mm_meta::mutations::indexing::IndexFileFromPathMutation {
                path: abs_path,
                zone: "corpus".to_string(),
            })
        })
        .collect()
}

/// Result of the pre-loop startup maintenance check.
#[derive(Default)]
pub(super) struct StartupMaintenanceCheck {
    pub needs_schema_update: bool,
    pub schema_descriptions: Vec<String>,
    pub needs_vacuum: bool,
}

/// Blocking startup check: schema reconciliation and vacuum (off main thread).
///
/// Called via `spawn_blocking().await` before the select loop starts. Opens
/// its own read-only DB connections for schema fingerprint comparison and
/// freelist ratio checks.
pub(super) fn startup_maintenance_check(vacuum_threshold: f64) -> StartupMaintenanceCheck {
    use crate::db::reconciler;

    let db_path = match config::get_db_path() {
        Ok(p) if p.exists() => p,
        _ => return StartupMaintenanceCheck::default(),
    };

    let db = match Database::open_read_only(&db_path) {
        Ok(d) => d,
        Err(_) => return StartupMaintenanceCheck::default(),
    };

    // Schema check
    let schema_dirty = !reconciler::fingerprint_matches(&db);
    let has_data_migrations = reconciler::has_pending_data_migrations(&db);

    let needs_schema = if schema_dirty || has_data_migrations {
        if schema_dirty {
            match reconciler::ReconciliationPlan::compute(db.conn()) {
                Ok(plan) => !plan.is_empty() || has_data_migrations,
                _ => true,
            }
        } else {
            true
        }
    } else {
        false
    };

    if needs_schema {
        let descriptions = match reconciler::ReconciliationPlan::compute_full(&db) {
            Ok(plan) => plan.descriptions(),
            Err(_) => vec!["Schema update needed (could not compute details)".to_string()],
        };
        return StartupMaintenanceCheck {
            needs_schema_update: true,
            schema_descriptions: descriptions,
            needs_vacuum: false,
        };
    }

    // Vacuum check (only if no schema update needed)
    drop(db); // Close the Database wrapper before raw connection
    let needs_vacuum = check_vacuum_needed_blocking(vacuum_threshold);

    StartupMaintenanceCheck {
        needs_schema_update: false,
        schema_descriptions: Vec::new(),
        needs_vacuum,
    }
}

/// Check if the database needs vacuuming based on freelist ratio (blocking).
fn check_vacuum_needed_blocking(vacuum_threshold: f64) -> bool {
    if vacuum_threshold <= 0.0 {
        return false;
    }

    let db_path = match config::get_db_path() {
        Ok(p) if p.exists() => p,
        _ => return false,
    };

    let conn = match rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    ) {
        Ok(c) => c,
        Err(_) => return false,
    };

    let page_count: u64 = conn
        .pragma_query_value(None, "page_count", |row| row.get(0))
        .unwrap_or(0);
    let freelist_count: u64 = conn
        .pragma_query_value(None, "freelist_count", |row| row.get(0))
        .unwrap_or(0);

    if page_count == 0 {
        return false;
    }

    let ratio = freelist_count as f64 / page_count as f64;
    ratio > vacuum_threshold
}
