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

    /// Automatically index unindexed corpus files after derivation.
    ///
    /// Called at the Inodes→Full transition when all derivation signal writes
    /// have been flushed. Reads UnindexedFileSignal rows, creates
    /// IndexFileFromPath mutations, and queues them for execution.
    ///
    /// Returns `true` if mutations were queued (caller should skip content
    /// analysis — the post-mutation re-derivation cycle handles it).
    pub(super) fn auto_index_unindexed_files(&mut self) -> bool {
        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return false,
        };
        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return false,
        };
        let read_db = crate::db::ReadOnlyDb::new(&db);

        let unindexed = match read_db.get_unindexed_signals_for::<crate::zones::CorpusZone>() {
            Ok(files) => files,
            Err(e) => {
                crate::logging::log_error(format!(
                    "[AUTO-INDEX] Failed to query unindexed signals: {}", e
                ));
                return false;
            }
        };

        if unindexed.is_empty() {
            return false;
        }

        let resolver = crate::corpus::paths::get_resolver();
        let mutations: Vec<Mutation> = unindexed
            .iter()
            .map(|(_inode, path)| {
                let abs_path = resolver.resolve_for_zone(
                    crate::db::types::Zone::Corpus,
                    std::path::Path::new(path),
                );
                Mutation::IndexFileFromPath(
                    mm_meta::mutations::indexing::IndexFileFromPathMutation {
                        path: abs_path,
                        zone: "corpus".to_string(),
                    },
                )
            })
            .collect();

        crate::logging::log_general(format!(
            "[AUTO-INDEX] Queueing {} index mutations for unindexed corpus files",
            mutations.len()
        ));

        self.queue_mutations_internal(
            mutations,
            Some("Auto-index unindexed files".to_string()),
        );
        self.files_indexed_this_cycle = true;
        true
    }

    // -------------------------------------------------------------------------
    // Migration-Aware Startup Methods
    // -------------------------------------------------------------------------

    /// Check if schema reconciliation is needed.
    ///
    /// Returns true if the database exists and has pending schema changes.
    pub fn needs_schema_update(&self) -> bool {
        use crate::db::reconciler;

        if self.startup_state != super::types::WitchStartupState::Ready {
            return false;
        }

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return false,
        };
        if !db_path.exists() {
            return false;
        }

        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return false,
        };

        let schema_dirty = !reconciler::fingerprint_matches(&db);
        let has_data_migrations = reconciler::has_pending_data_migrations(&db);

        if !schema_dirty && !has_data_migrations {
            return false;
        }

        if schema_dirty {
            match reconciler::ReconciliationPlan::compute(db.conn()) {
                Ok(plan) => !plan.is_empty() || has_data_migrations,
                _ => true,
            }
        } else {
            true
        }
    }

    /// Get pending schema update descriptions for UI display.
    ///
    /// Returns a list of human-readable descriptions of planned changes.
    pub fn pending_schema_descriptions(&self) -> Vec<String> {
        use crate::db::reconciler::ReconciliationPlan;

        let db_path = match config::get_db_path() {
            Ok(p) => p,
            Err(_) => return Vec::new(),
        };
        let db = match Database::open_read_only(&db_path) {
            Ok(d) => d,
            Err(_) => return Vec::new(),
        };

        match ReconciliationPlan::compute_full(&db) {
            Ok(plan) => plan.descriptions(),
            Err(_) => vec!["Schema update needed (could not compute details)".to_string()],
        }
    }

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

    /// Check if the database needs vacuuming based on freelist ratio.
    ///
    /// Returns true if the freelist ratio exceeds `self.vacuum_threshold`.
    pub(super) fn check_vacuum_needed(&self) -> bool {
        if self.vacuum_threshold <= 0.0 {
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
        ratio > self.vacuum_threshold
    }
}
