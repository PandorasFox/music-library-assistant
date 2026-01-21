//! Health signal and known variant operations.
//!
//! Signals are facts about corpus state. They are created by computations and
//! deleted when they become stale. There is no "resolution" concept - signals
//! simply exist or don't exist based on current corpus state.
//!
//! ## Witnessed Operations
//!
//! Signal-altering operations require a `ComputationWitness` to ensure they're
//! only called from computation execution contexts. Use:
//! - `ensure_signal` - idempotent create (no-op if exists)
//! - `clear_signal` - idempotent delete (no-op if doesn't exist)
//! - `replace_signal` - delete existing + insert new (for summary signals)

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Database;
use crate::corpus::computations::ComputationWitness;
use crate::corpus::db::types::{
    AggregateSignal, AggregateSignalType, CorpusSummary, FileSignalType, HealthIssue,
    HealthIssueType, HealthSummary, KnownVariant, Track, VariantType,
};

impl Database {
    // ========================================================================
    // Corpus Summary
    // ========================================================================

    /// Get aggregated corpus summary for UI display.
    pub fn get_corpus_summary(&self) -> Result<CorpusSummary> {
        let track_count = self.get_track_count(Some("corpus")).unwrap_or(0);

        // Count deploy conflict signals
        let deploy_conflicts: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'deploy_conflict'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        // File-level signal counts (benign signals - shown separately)
        let files_in_corpus: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'file_in_corpus'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let healthy_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'healthy_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let unindexed_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'unindexed_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let missing_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'missing_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let moved_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'moved_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let library_stale: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'library_stale'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let library_orphan: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'library_orphan'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let modified_oob: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'corpus_file_modified_out_of_band'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let tags_changed_oob: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'out_of_band_tag_change'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let duplicate_inodes: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM health_issues WHERE issue_type = 'duplicate_inode'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        // Get total health issue count excluding:
        // - deploy_conflicts (shown separately)
        // - file_in_corpus, healthy_file (benign status signals)
        // - unindexed_file, missing_file, moved_file (file-level signals shown separately)
        let total_health_issues: usize = self.conn.query_row(
            r#"SELECT COUNT(*) FROM health_issues
               WHERE issue_type NOT IN (
                   'deploy_conflict',
                   'file_in_corpus',
                   'healthy_file',
                   'unindexed_file',
                   'missing_file',
                   'moved_file'
               )"#,
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let mut health_summary = self.get_health_summary().unwrap_or_default();
        // Set total count from direct query (excludes benign/file-level signals)
        health_summary.total_issues = total_health_issues;

        let deployment_stats = self.get_deployment_stats().ok().flatten();
        let pending_changes = std::collections::HashMap::new(); // Decisions now in-memory only

        // Get last scan time from scan_history
        let last_scan: Option<String> = self
            .conn
            .query_row(
                "SELECT completed_at FROM scan_history WHERE source = 'corpus' ORDER BY id DESC LIMIT 1",
                params![],
                |row| row.get(0),
            )
            .optional()
            .ok()
            .flatten();

        Ok(CorpusSummary {
            track_count,
            deploy_conflicts,
            health_summary,
            deployment_stats,
            pending_changes,
            last_scan,
            files_in_corpus,
            healthy_files,
            unindexed_files,
            missing_files,
            moved_files,
            library_stale,
            library_orphan,
            modified_oob,
            tags_changed_oob,
            duplicate_inodes,
        })
    }

    // ========================================================================
    // Health Issue Operations
    // ========================================================================

    /// Insert a new health signal.
    pub fn insert_health_issue(&self, issue: &HealthIssue) -> Result<i64> {
        self.conn
            .execute(
                r#"
                INSERT INTO health_issues
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, COALESCE(?3, CURRENT_TIMESTAMP), ?4)
                "#,
                params![
                    issue.issue_type.as_str(),
                    &issue.issue_key,
                    &issue.discovered_at,
                    &issue.metadata_json,
                ],
            )
            .context("Failed to insert health signal")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get health signals, optionally filtered by type.
    pub fn get_health_signals(
        &self,
        issue_type: Option<HealthIssueType>,
    ) -> Result<Vec<HealthIssue>> {
        let sql = match issue_type {
            Some(_) => {
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
                   FROM health_issues
                   WHERE issue_type = ?1
                   ORDER BY discovered_at DESC"#
            }
            None => {
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
                   FROM health_issues
                   ORDER BY discovered_at DESC"#
            }
        };

        let mut stmt = self.conn.prepare(sql)?;

        let rows = if let Some(it) = issue_type {
            stmt.query_map(params![it.as_str()], Self::row_to_health_issue)?
        } else {
            stmt.query_map(params![], Self::row_to_health_issue)?
        };

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }


    /// Get health signal by type and key (e.g., path or fingerprint).
    pub fn get_health_issue_by_key(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
    ) -> Result<Option<HealthIssue>> {
        self.conn
            .query_row(
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
                   FROM health_issues
                   WHERE issue_type = ?1 AND issue_key = ?2"#,
                params![issue_type.as_str(), issue_key],
                Self::row_to_health_issue,
            )
            .optional()
            .context("Failed to query health signal by key")
    }

    /// Fast existence check for a signal (no data fetch).
    ///
    /// Use this before emitting signals to avoid redundant DB writes.
    pub fn signal_exists(&self, issue_type: HealthIssueType, issue_key: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2 LIMIT 1",
                params![issue_type.as_str(), issue_key],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Fast existence check for a file signal type.
    ///
    /// Convenience wrapper using FileSignalType's string representation.
    pub fn file_signal_exists(&self, signal_type: FileSignalType, key: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2 LIMIT 1",
                params![signal_type.as_str(), key],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Delete a health signal by ID.
    pub fn delete_health_signal(&self, signal_id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM health_issues WHERE id = ?1", params![signal_id])
            .context("Failed to delete health signal")?;
        Ok(())
    }

    /// Delete health signals by type and key.
    pub fn delete_health_signals_by_key(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
    ) -> Result<usize> {
        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2",
                params![issue_type.as_str(), issue_key],
            )
            .context("Failed to delete health signals")?;
        Ok(deleted)
    }

    /// Delete all signals of a given type for a specific path.
    pub fn delete_signals_for_path(&self, path: &str) -> Result<usize> {
        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_key = ?1",
                params![path],
            )
            .context("Failed to delete signals for path")?;
        Ok(deleted)
    }

    // ========================================================================
    // Witnessed Signal Operations (require ComputationWitness)
    // ========================================================================

    /// Ensure a signal exists (idempotent create).
    ///
    /// Creates the signal if it doesn't exist; does nothing if it already exists.
    /// Returns `true` if a new signal was created, `false` if it already existed.
    ///
    /// Requires `ComputationWitness` to ensure this is called from computation context.
    pub fn ensure_signal(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
        metadata_json: Option<&str>,
        _witness: &ComputationWitness,
    ) -> Result<bool> {
        // Single-statement idempotent insert using UNIQUE constraint
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO health_issues
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, CURRENT_TIMESTAMP, ?3)
                "#,
                params![
                    issue_type.as_str(),
                    issue_key,
                    metadata_json,
                ],
            )
            .context("Failed to ensure signal")?;

        Ok(self.conn.changes() > 0)
    }

    /// Clear a signal if it exists (idempotent delete).
    ///
    /// Deletes the signal if it exists; does nothing if it doesn't exist.
    /// Returns `true` if a signal was deleted, `false` if none existed.
    ///
    /// Requires `ComputationWitness` to ensure this is called from computation context.
    pub fn clear_signal(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
        _witness: &ComputationWitness,
    ) -> Result<bool> {
        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2",
                params![issue_type.as_str(), issue_key],
            )
            .context("Failed to clear signal")?;

        Ok(deleted > 0)
    }

    /// Replace a signal (delete existing + insert new).
    ///
    /// Used for signals like LibraryHealthSummary where we want to update
    /// with fresh data rather than accumulate.
    ///
    /// Requires `ComputationWitness` to ensure this is called from computation context.
    pub fn replace_signal(
        &self,
        issue: &HealthIssue,
        _witness: &ComputationWitness,
    ) -> Result<i64> {
        // Delete existing signal with same type and key
        self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2",
                params![issue.issue_type.as_str(), &issue.issue_key],
            )
            .context("Failed to delete existing signal")?;

        // Insert new signal
        self.conn
            .execute(
                r#"
                INSERT INTO health_issues
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, COALESCE(?3, CURRENT_TIMESTAMP), ?4)
                "#,
                params![
                    issue.issue_type.as_str(),
                    &issue.issue_key,
                    &issue.discovered_at,
                    &issue.metadata_json,
                ],
            )
            .context("Failed to replace signal")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Clear all signals of a specific type for a directory.
    ///
    /// Used before recomputing signals for a directory to ensure stale signals
    /// are removed.
    ///
    /// Requires `ComputationWitness` to ensure this is called from computation context.
    pub fn clear_signals_in_directory(
        &self,
        directory: &std::path::Path,
        issue_type: HealthIssueType,
        _witness: &ComputationWitness,
    ) -> Result<usize> {
        let pattern = super::dir_like_pattern(directory);

        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key LIKE ?2 ESCAPE '\\'",
                params![issue_type.as_str(), pattern],
            )
            .context("Failed to clear signals in directory")?;

        Ok(deleted)
    }

    // ========================================================================
    // Type-Safe Signal Operations (File and Aggregate)
    // ========================================================================

    /// Ensure a file signal exists (idempotent, no metadata).
    pub fn ensure_file_signal(
        &self,
        signal_type: FileSignalType,
        path: &str,
        _witness: &ComputationWitness,
    ) -> Result<bool> {
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO health_issues
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, CURRENT_TIMESTAMP, NULL)
                "#,
                params![signal_type.as_str(), path],
            )
            .context("Failed to ensure file signal")?;

        Ok(self.conn.changes() > 0)
    }

    /// Clear a file signal (idempotent delete).
    pub fn clear_file_signal(
        &self,
        signal_type: FileSignalType,
        path: &str,
        _witness: &ComputationWitness,
    ) -> Result<bool> {
        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2",
                params![signal_type.as_str(), path],
            )
            .context("Failed to clear file signal")?;

        Ok(deleted > 0)
    }

    /// Clear all file signals of a type in a directory.
    pub fn clear_file_signals_in_directory(
        &self,
        directory: &std::path::Path,
        signal_type: FileSignalType,
        _witness: &ComputationWitness,
    ) -> Result<usize> {
        let pattern = super::dir_like_pattern(directory);

        let deleted = self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key LIKE ?2 ESCAPE '\\'",
                params![signal_type.as_str(), pattern],
            )
            .context("Failed to clear file signals in directory")?;

        Ok(deleted)
    }

    /// Ensure an aggregate signal exists (with metadata).
    pub fn ensure_aggregate_signal(
        &self,
        signal_type: AggregateSignalType,
        key: &str,
        metadata_json: Option<&str>,
        _witness: &ComputationWitness,
    ) -> Result<bool> {
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO health_issues
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, CURRENT_TIMESTAMP, ?3)
                "#,
                params![signal_type.as_str(), key, metadata_json],
            )
            .context("Failed to ensure aggregate signal")?;

        Ok(self.conn.changes() > 0)
    }

    /// Replace an aggregate signal (delete + insert).
    pub fn replace_aggregate_signal(
        &self,
        signal: &AggregateSignal,
        _witness: &ComputationWitness,
    ) -> Result<i64> {
        self.conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2",
                params![signal.signal_type.as_str(), &signal.key],
            )
            .context("Failed to delete existing aggregate signal")?;

        self.conn
            .execute(
                r#"
                INSERT INTO health_issues
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, COALESCE(?3, CURRENT_TIMESTAMP), ?4)
                "#,
                params![
                    signal.signal_type.as_str(),
                    &signal.key,
                    &signal.discovered_at,
                    &signal.metadata_json,
                ],
            )
            .context("Failed to replace aggregate signal")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Clear an aggregate signal (idempotent delete).
    pub fn clear_aggregate_signal(
        &self,
        signal_type: AggregateSignalType,
        key: &str,
        _witness: &ComputationWitness,
    ) -> Result<bool> {
        let deleted = self
            .conn
            .execute(
                "DELETE FROM health_issues WHERE issue_type = ?1 AND issue_key = ?2",
                params![signal_type.as_str(), key],
            )
            .context("Failed to clear aggregate signal")?;

        Ok(deleted > 0)
    }

    /// Get all aggregate signal keys and their metadata for a given type.
    ///
    /// Returns (key, metadata_json) pairs for set-difference computations.
    pub fn get_aggregate_signal_keys_with_metadata(
        &self,
        signal_type: AggregateSignalType,
    ) -> Result<Vec<(String, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT issue_key, metadata_json FROM health_issues WHERE issue_type = ?1",
        )?;

        let rows = stmt.query_map(params![signal_type.as_str()], |row| {
            let key: String = row.get(0)?;
            let metadata: Option<String> = row.get(1)?;
            Ok((key, metadata))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // ========================================================================
    // Directory-Level Queries (for chunked computations)
    // ========================================================================

    /// Get distinct parent directories from tracks table.
    pub fn get_distinct_track_directories(&self) -> Result<Vec<std::path::PathBuf>> {
        use std::path::PathBuf;

        // Fetch all paths and compute parent directories in Rust
        let mut stmt = self.conn.prepare("SELECT DISTINCT path FROM tracks")?;
        let rows = stmt.query_map(params![], |row| {
            let path: String = row.get(0)?;
            Ok(path)
        })?;

        let mut directories = std::collections::HashSet::new();
        for row in rows {
            let path = row?;
            if let Some(parent) = PathBuf::from(&path).parent() {
                directories.insert(parent.to_path_buf());
            }
        }

        Ok(directories.into_iter().collect())
    }

    /// Get distinct parent directories from FileInCorpus signals.
    ///
    /// FileInCorpus signals use the file path as issue_key.
    /// This extracts and deduplicates the parent directories.
    pub fn get_distinct_corpus_directories(&self) -> Result<Vec<std::path::PathBuf>> {
        use std::path::PathBuf;

        // FileInCorpus signals use file path as issue_key
        let mut stmt = self.conn.prepare(
            r#"SELECT DISTINCT issue_key FROM health_issues
               WHERE issue_type = 'file_in_corpus'"#
        )?;

        let rows = stmt.query_map(params![], |row| {
            let path: String = row.get(0)?;
            Ok(path)
        })?;

        let mut directories = std::collections::HashSet::new();
        for row in rows {
            let path = row?;
            // Extract parent directory from file path
            if let Some(parent) = PathBuf::from(&path).parent() {
                directories.insert(parent.to_path_buf());
            }
        }

        Ok(directories.into_iter().collect())
    }

    /// Get signals in a specific directory.
    ///
    /// For FileInCorpus signals, issue_key is the file path.
    /// Uses LIKE pattern with trailing slash to avoid matching sibling directories
    /// (e.g., `/path/to/dir/` won't match `/path/to/dir-extra/file.mp3`).
    pub fn get_signals_in_directory(
        &self,
        dir: &std::path::Path,
        signal_type: HealthIssueType,
    ) -> Result<Vec<HealthIssue>> {
        let pattern = super::dir_like_pattern(dir);

        let mut stmt = self.conn.prepare(
            r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
               FROM health_issues
               WHERE issue_type = ?1 AND issue_key LIKE ?2 ESCAPE '\'"#
        )?;

        let rows = stmt.query_map(
            params![signal_type.as_str(), pattern],
            Self::row_to_health_issue
        )?;

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    // Note: get_tracks_in_directory_with_fingerprint is defined in tracks.rs

    /// Get signals discovered since a given timestamp.
    pub fn get_signals_since(
        &self,
        signal_type: HealthIssueType,
        since: &str,
    ) -> Result<Vec<HealthIssue>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
               FROM health_issues
               WHERE issue_type = ?1 AND discovered_at > ?2
               ORDER BY discovered_at DESC"#
        )?;

        let rows = stmt.query_map(
            params![signal_type.as_str(), since],
            Self::row_to_health_issue
        )?;

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    /// Get tracks associated with an aggregate signal (from embedded track_ids in metadata_json).
    ///
    /// Aggregate signals store track IDs directly in metadata_json["track_ids"].
    pub fn get_aggregate_signal_tracks(&self, signal_id: i64) -> Result<Vec<Track>> {
        // Get the signal to extract track_ids from metadata
        let signal: HealthIssue = self.conn.query_row(
            r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
               FROM health_issues WHERE id = ?1"#,
            params![signal_id],
            Self::row_to_health_issue,
        ).context("Signal not found")?;

        // Parse track_ids from metadata_json
        let track_ids: Vec<i64> = signal
            .metadata_json
            .as_ref()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
            .and_then(|v| v.get("track_ids").cloned())
            .and_then(|v| v.as_array().cloned())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();

        if track_ids.is_empty() {
            return Ok(Vec::new());
        }

        // Batch fetch tracks by ID
        self.get_tracks_by_ids(&track_ids)
    }

    /// Get health summary statistics.
    pub fn get_health_summary(&self) -> Result<HealthSummary> {
        let mut summary = HealthSummary::default();

        // Count by issue type
        let mut stmt = self.conn.prepare(
            r#"SELECT issue_type, COUNT(*)
               FROM health_issues
               GROUP BY issue_type"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            let issue_type: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((issue_type, count as usize))
        })?;

        for row in rows {
            let (issue_type, count) = row?;

            match issue_type.as_str() {
                "fingerprint_dup" => summary.fingerprint_duplicates += count,
                "metadata_dup" => summary.metadata_duplicates += count,
                // Legacy and unified type both map to canonicalization_issues
                "canon" | "tag_canon" | "genre_canon" => summary.canonicalization_issues += count,
                "missing_tag" => summary.missing_tag_issues += count,
                _ => {}
            }
        }

        // Count known variants
        summary.known_variants = self.conn.query_row(
            "SELECT COUNT(*) FROM known_variants",
            params![],
            |row| row.get(0),
        )?;

        // Count unconfirmed tag canonicalizations as canonicalization issues
        // These are stored in tag_canonicalization, not health_issues
        let unconfirmed_canons: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM tag_canonicalization WHERE confirmed_at IS NULL",
            params![],
            |row| row.get(0),
        )?;
        summary.canonicalization_issues += unconfirmed_canons;

        Ok(summary)
    }

    // ========================================================================
    // Known Variant Operations
    // ========================================================================

    /// Insert a known variant relationship.
    pub fn insert_known_variant(&self, variant: &KnownVariant) -> Result<i64> {
        self.conn
            .execute(
                r#"INSERT INTO known_variants
                   (variant_type, canonical_fingerprint, variant_fingerprint,
                    canonical_track_id, variant_track_id, marked_at, notes)
                   VALUES (?1, ?2, ?3, ?4, ?5, COALESCE(?6, CURRENT_TIMESTAMP), ?7)"#,
                params![
                    variant.variant_type.as_str(),
                    &variant.canonical_fingerprint,
                    &variant.variant_fingerprint,
                    variant.canonical_track_id,
                    variant.variant_track_id,
                    &variant.marked_at,
                    &variant.notes,
                ],
            )
            .context("Failed to insert known variant")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Check if a fingerprint is a known variant.
    pub fn is_known_variant(&self, fingerprint: &str) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            r#"SELECT COUNT(*) FROM known_variants
               WHERE canonical_fingerprint = ?1 OR variant_fingerprint = ?1"#,
            params![fingerprint],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get known variants for a fingerprint.
    pub fn get_known_variants_for_fingerprint(&self, fingerprint: &str) -> Result<Vec<KnownVariant>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, variant_type, canonical_fingerprint, variant_fingerprint,
                      canonical_track_id, variant_track_id, marked_at, notes
               FROM known_variants
               WHERE canonical_fingerprint = ?1 OR variant_fingerprint = ?1"#,
        )?;

        let rows = stmt.query_map(params![fingerprint], Self::row_to_known_variant)?;

        let mut variants = Vec::new();
        for row in rows {
            variants.push(row?);
        }
        Ok(variants)
    }

    /// Get all known variants.
    pub fn get_all_known_variants(&self) -> Result<Vec<KnownVariant>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, variant_type, canonical_fingerprint, variant_fingerprint,
                      canonical_track_id, variant_track_id, marked_at, notes
               FROM known_variants
               ORDER BY variant_type, canonical_fingerprint"#,
        )?;

        let rows = stmt.query_map(params![], Self::row_to_known_variant)?;

        let mut variants = Vec::new();
        for row in rows {
            variants.push(row?);
        }
        Ok(variants)
    }

    // ========================================================================
    // Row Conversion Helpers
    // ========================================================================

    /// Convert a row to HealthIssue.
    /// Expected columns: id, issue_type, issue_key, discovered_at, metadata_json
    pub(super) fn row_to_health_issue(row: &rusqlite::Row) -> rusqlite::Result<HealthIssue> {
        let issue_type_str: String = row.get(1)?;

        Ok(HealthIssue {
            id: Some(row.get(0)?),
            issue_type: HealthIssueType::from_str(&issue_type_str)
                .unwrap_or(HealthIssueType::FingerprintDuplicate),
            issue_key: row.get(2)?,
            discovered_at: row.get(3)?,
            metadata_json: row.get(4)?,
        })
    }

    pub(super) fn row_to_known_variant(row: &rusqlite::Row) -> rusqlite::Result<KnownVariant> {
        let variant_type_str: String = row.get(1)?;

        Ok(KnownVariant {
            id: Some(row.get(0)?),
            variant_type: VariantType::from_str(&variant_type_str)
                .unwrap_or(VariantType::Rerelease),
            canonical_fingerprint: row.get(2)?,
            variant_fingerprint: row.get(3)?,
            canonical_track_id: row.get(4)?,
            variant_track_id: row.get(5)?,
            marked_at: row.get(6)?,
            notes: row.get(7)?,
        })
    }
}
