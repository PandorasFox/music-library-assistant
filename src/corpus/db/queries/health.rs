//! Health signal and known variant operations.
//!
//! Signals are facts about corpus state. They are created by computations and
//! deleted when they become stale. There is no "resolution" concept - signals
//! simply exist or don't exist based on current corpus state.
//!
//! ## Witnessed Operations
//!
//! Signal-altering operations require a witness (`ComputationWitness` or
//! `MutationExecutionWitness`) to ensure they're only called from authorized
//! execution contexts. Use:
//! - `ensure_signal` - idempotent create (no-op if exists)
//! - `clear_signal` - idempotent delete (no-op if doesn't exist)
//! - `replace_signal` - delete existing + insert new (for summary signals)

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};

use super::Database;
use crate::db_thread::SignalWitness;
use crate::corpus::db::types::{
    AggregateSignal, AggregateSignalType, CorpusSummary, FileSignalType, FileSource, Signal,
    SignalType, SignalSummary,
};

impl Database {
    // ========================================================================
    // Corpus Summary
    // ========================================================================

    /// Get aggregated corpus summary for UI display.
    pub fn get_corpus_summary(&self) -> Result<CorpusSummary> {
        let track_count = self.get_audio_file_count(Some("corpus")).unwrap_or(0);

        // Count deploy conflict signals
        let deploy_conflicts: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'deploy_conflict'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        // File-level signal counts (benign signals - shown separately)
        let files_in_corpus: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'file_in_corpus'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let healthy_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'healthy_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let unindexed_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'unindexed_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let missing_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'missing_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let missing_directories: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'missing_directory'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let moved_files: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'moved_file'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let library_stale: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'library_stale'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let library_leftover: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'library_leftover'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let oob_tag_sync: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'oob_tag_sync'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let oob_tag_conflict: usize = self.conn.query_row(
            // Include legacy "oob_tag" in conflict count for transition
            "SELECT COUNT(*) FROM signals WHERE issue_type IN ('oob_tag_conflict', 'oob_tag')",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let mtime_only_mismatch: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'mtime_only_mismatch'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let duplicate_inodes: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = 'duplicate_inode'",
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        // Get total health issue count excluding:
        // - deploy_conflicts (shown separately)
        // - file_in_corpus, healthy_file (benign status signals)
        // - unindexed_file, missing_file, missing_directory, moved_file (file-level signals shown separately)
        let total_signals: usize = self.conn.query_row(
            r#"SELECT COUNT(*) FROM signals
               WHERE issue_type NOT IN (
                   'deploy_conflict',
                   'file_in_corpus',
                   'healthy_file',
                   'unindexed_file',
                   'missing_file',
                   'missing_directory',
                   'moved_file'
               )"#,
            params![],
            |row| row.get(0),
        ).unwrap_or(0);

        let mut signal_summary = self.get_signal_summary().unwrap_or_default();
        // Set total count from direct query (excludes benign/file-level signals)
        signal_summary.total_issues = total_signals;

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
            _track_count: track_count,
            deploy_conflicts,
            signal_summary,
            _deployment_stats: deployment_stats,
            _pending_changes: pending_changes,
            _last_scan: last_scan,
            files_in_corpus,
            healthy_files,
            unindexed_files,
            missing_files,
            missing_directories,
            moved_files,
            library_stale,
            library_leftover,
            oob_tag_sync,
            oob_tag_conflict,
            mtime_only_mismatch,
            duplicate_inodes,
        })
    }

    // ========================================================================
    // Health Issue Operations
    // ========================================================================

    /// Get health signals, optionally filtered by type.
    pub fn get_signals(
        &self,
        issue_type: Option<SignalType>,
    ) -> Result<Vec<Signal>> {
        let sql = match issue_type {
            Some(_) => {
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
                   FROM signals
                   WHERE issue_type = ?1
                   ORDER BY discovered_at DESC"#
            }
            None => {
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
                   FROM signals
                   ORDER BY discovered_at DESC"#
            }
        };

        let mut stmt = self.conn.prepare(sql)?;

        let rows = if let Some(it) = issue_type {
            stmt.query_map(params![it.as_str()], Self::row_to_signal)?
        } else {
            stmt.query_map(params![], Self::row_to_signal)?
        };

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }


    /// Get signal by ID.
    pub fn get_signal_by_id(&self, signal_id: i64) -> Result<Option<Signal>> {
        self.conn
            .query_row(
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
                   FROM signals
                   WHERE id = ?1"#,
                params![signal_id],
                Self::row_to_signal,
            )
            .optional()
            .context("Failed to query signal by ID")
    }

    /// Fast existence check for a file signal type.
    ///
    /// Convenience wrapper using FileSignalType's string representation.
    pub fn file_signal_exists(&self, signal_type: FileSignalType, key: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM signals WHERE issue_type = ?1 AND issue_key = ?2 LIMIT 1",
                params![signal_type.as_str(), key],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Delete all signals for a specific path.
    ///
    /// Used during track deletion to clear all associated signals.
    pub fn delete_signals_for_path(&self, path: &str, _witness: &impl SignalWitness) -> Result<usize> {
        let deleted = self.conn
            .execute(
                "DELETE FROM signals WHERE issue_key = ?1",
                params![path],
            )
            .context("Failed to delete signals for path")?;
        Ok(deleted)
    }

    /// Delete mutable signals for a path, preserving file-inherent signals.
    ///
    /// File-inherent signals (CorruptFile, ShitFormat) are properties of the file itself
    /// and should only be cleared by specific mutations (MoveToStash, DropFromIndex, Transcode)
    /// or by verification computations (VerifyAudio).
    ///
    /// Tag-based signals (OOB conflicts, mtime mismatches, health status) can be cleared
    /// and recomputed by UpdateCorpusFileSignals.
    ///
    /// TODO: Refactor to use signal categories at the type level instead of SQL string matching.
    /// Consider a SignalCategory enum (FileInherent, TagBased, Aggregate) with methods to
    /// determine clearing behavior.
    pub fn delete_mutable_signals_for_path(&self, path: &str, _witness: &impl SignalWitness) -> Result<usize> {
        let deleted = self.conn
            .execute(
                "DELETE FROM signals WHERE issue_key = ?1 AND issue_type NOT IN ('corrupt_file', 'shit_format')",
                params![path],
            )
            .context("Failed to delete mutable signals for path")?;
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
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO signals
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, CURRENT_TIMESTAMP, NULL)
                "#,
                params![signal_type.as_str(), path],
            )
            .context("Failed to ensure file signal")?;

        Ok(self.conn.changes() > 0)
    }

    /// Ensure a file signal exists with metadata (idempotent).
    ///
    /// Works for all FileSignalType variants including DeployReady/DeployedHealthy.
    pub fn ensure_file_signal_with_metadata(
        &self,
        signal_type: FileSignalType,
        key: &str,
        metadata_json: Option<&str>,
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO signals
                (issue_type, issue_key, discovered_at, metadata_json)
                VALUES (?1, ?2, CURRENT_TIMESTAMP, ?3)
                "#,
                params![signal_type.as_str(), key, metadata_json],
            )
            .context("Failed to ensure file signal with metadata")?;

        Ok(self.conn.changes() > 0)
    }

    /// Clear a file signal (idempotent delete).
    pub fn clear_file_signal(
        &self,
        signal_type: FileSignalType,
        path: &str,
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        let deleted = self.conn
            .execute(
                "DELETE FROM signals WHERE issue_type = ?1 AND issue_key = ?2",
                params![signal_type.as_str(), path],
            )
            .context("Failed to clear file signal")?;

        Ok(deleted > 0)
    }

    /// Ensure an aggregate signal exists (with metadata).
    pub fn ensure_aggregate_signal(
        &self,
        signal_type: AggregateSignalType,
        key: &str,
        metadata_json: Option<&str>,
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO signals
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
        _witness: &impl SignalWitness,
    ) -> Result<i64> {
        self.conn
            .execute(
                "DELETE FROM signals WHERE issue_type = ?1 AND issue_key = ?2",
                params![signal.signal_type.as_str(), &signal.key],
            )
            .context("Failed to delete existing aggregate signal")?;

        self.conn
            .execute(
                r#"
                INSERT INTO signals
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
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        let deleted = self
            .conn
            .execute(
                "DELETE FROM signals WHERE issue_type = ?1 AND issue_key = ?2",
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
            "SELECT issue_key, metadata_json FROM signals WHERE issue_type = ?1",
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

    /// Get all aggregate signals of a given type.
    ///
    /// Returns full AggregateSignal structs for UI display and modal data loading.
    pub fn get_aggregate_signals(
        &self,
        signal_type: Option<AggregateSignalType>,
    ) -> Result<Vec<AggregateSignal>> {
        let sql = match signal_type {
            Some(_) => {
                "SELECT id, issue_type, issue_key, discovered_at, metadata_json
                 FROM signals WHERE issue_type = ?1 ORDER BY discovered_at DESC"
            }
            None => {
                "SELECT id, issue_type, issue_key, discovered_at, metadata_json
                 FROM signals ORDER BY discovered_at DESC"
            }
        };

        let mut stmt = self.conn.prepare(sql)?;

        let row_mapper = |row: &rusqlite::Row| {
            let id: i64 = row.get(0)?;
            let type_str: String = row.get(1)?;
            let key: String = row.get(2)?;
            let discovered_at: Option<String> = row.get(3)?;
            let metadata_json: Option<String> = row.get(4)?;

            let signal_type = AggregateSignalType::from_str(&type_str)
                .unwrap_or(AggregateSignalType::FingerprintOverlap);

            Ok(AggregateSignal {
                id: Some(id),
                signal_type,
                key,
                discovered_at,
                metadata_json,
            })
        };

        let rows = if let Some(t) = signal_type {
            stmt.query_map(params![t.as_str()], row_mapper)?
        } else {
            stmt.query_map(params![], row_mapper)?
        };

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // ========================================================================
    // Directory-Level Queries (for chunked computations)
    // ========================================================================

    /// Get distinct parent directories from corpus audio files (files + audio_info).
    pub fn get_distinct_track_directories(&self) -> Result<Vec<std::path::PathBuf>> {
        use std::path::PathBuf;

        // Fetch all corpus audio file paths and compute parent directories in Rust
        let mut stmt = self.conn.prepare(
            r#"SELECT DISTINCT f.path FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.is_dir = 0 AND f.source = 'corpus'"#
        )?;
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

    /// Get indexed corpus directories (directories stored in files table).
    ///
    /// Returns directory paths from the files table where is_dir=1 and source='corpus'.
    /// Used to detect missing directories (directories that were indexed but no longer exist).
    pub fn get_indexed_corpus_directories(&self) -> Result<Vec<std::path::PathBuf>> {
        use std::path::PathBuf;

        let mut stmt = self.conn.prepare(
            r#"SELECT path FROM files
               WHERE is_dir = 1 AND source = 'corpus'"#
        )?;
        let rows = stmt.query_map(params![], |row| {
            let path: String = row.get(0)?;
            Ok(PathBuf::from(path))
        })?;

        let mut directories = Vec::new();
        for row in rows {
            directories.push(row?);
        }

        Ok(directories)
    }

    /// Get missing directory signal paths (for UI resolution modal).
    pub fn get_missing_directory_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT issue_key FROM signals WHERE issue_type = 'missing_directory' ORDER BY issue_key"
        )?;
        let rows = stmt.query_map(params![], |row| row.get(0))?;

        let mut paths = Vec::new();
        for row in rows {
            paths.push(row?);
        }

        Ok(paths)
    }

    /// Get distinct parent directories from FileInCorpus signals.
    ///
    /// FileInCorpus signals use the file path as issue_key.
    /// This extracts and deduplicates the parent directories.
    pub fn get_distinct_corpus_directories(&self) -> Result<Vec<std::path::PathBuf>> {
        use std::path::PathBuf;

        // FileInCorpus signals use file path as issue_key
        let mut stmt = self.conn.prepare(
            r#"SELECT DISTINCT issue_key FROM signals
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
        signal_type: SignalType,
    ) -> Result<Vec<Signal>> {
        let pattern = super::dir_like_pattern(dir);

        let mut stmt = self.conn.prepare(
            r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
               FROM signals
               WHERE issue_type = ?1 AND issue_key LIKE ?2 ESCAPE '\'"#
        )?;

        let rows = stmt.query_map(
            params![signal_type.as_str(), pattern],
            Self::row_to_signal
        )?;

        let mut issues = Vec::new();
        for row in rows {
            issues.push(row?);
        }
        Ok(issues)
    }

    /// Get health summary statistics.
    pub fn get_signal_summary(&self) -> Result<SignalSummary> {
        let mut summary = SignalSummary::default();

        // Count by issue type
        let mut stmt = self.conn.prepare(
            r#"SELECT issue_type, COUNT(*)
               FROM signals
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
                "metadata_dup" => summary.metadata_duplicates += count,
                // Legacy and unified type both map to canonicalization_issues
                "canon" | "tag_canon" | "genre_canon" => summary.canonicalization_issues += count,
                "missing_tag" => summary.missing_tag_issues += count,
                _ => {}
            }
        }

        // Count unconfirmed tag canonicalizations as canonicalization issues
        // These are stored in tag_canonicalization, not signals
        let unconfirmed_canons: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM tag_canonicalization WHERE confirmed_at IS NULL",
            params![],
            |row| row.get(0),
        )?;
        summary.canonicalization_issues += unconfirmed_canons;

        Ok(summary)
    }

    // ========================================================================
    // Insights Data
    // ========================================================================

    /// Get InsightsData for the Insights view.
    ///
    /// Computes all bucket data via SQL queries. Called by UiReadCache.
    pub fn get_insights_data(&self) -> Result<crate::corpus::db::types::InsightsData> {
        use crate::corpus::db::types::*;

        Ok(InsightsData {
            bucket_corpus: self.compute_corpus_files_bucket()?,
            bucket_placeholder: self.compute_tag_resolution_bucket()?,
            bucket_library: self.compute_library_deploy_bucket()?,
            bucket_other: self.compute_other_signals_bucket()?,
        })
    }

    fn compute_corpus_files_bucket(&self) -> Result<crate::corpus::db::types::CorpusFilesBucket> {
        use crate::corpus::db::types::*;

        // OOB signals (highest priority)
        let oob_tag_sync = self.count_signal_type("oob_tag_sync")?;
        // Include legacy "oob_tag" in conflict count for transition
        let oob_tag_conflict = self.count_signal_type("oob_tag_conflict")?
            + self.count_signal_type("oob_tag").unwrap_or(0);
        let mtime_only_mismatch = self.count_signal_type("mtime_only_mismatch")?;
        let inode_changed = self.count_signal_type("inode_changed")?;

        // Standard corpus file signals
        let files_in_corpus = self.count_signal_type("file_in_corpus")?;
        let files_indexed = self.get_audio_file_count(Some("corpus")).unwrap_or(0);
        let files_unindexed = self.count_signal_type("unindexed_file")?;
        let files_missing = self.count_signal_type("missing_file")?;
        let directories_missing = self.count_signal_type("missing_directory")?;
        let files_relocated = self.count_signal_type("moved_file")?;

        // Error/format signals
        let corrupt_files = self.count_signal_type("corrupt_file")?;
        let shit_format_files = self.count_signal_type("shit_format")?;

        // File type breakdown
        let file_type_breakdown = self.get_file_type_breakdown()?;

        // Directory breakdown for FileInCorpus signals
        let directory_breakdown = self.get_directory_breakdown("file_in_corpus")?;

        Ok(CorpusFilesBucket {
            oob_tag_sync,
            oob_tag_conflict,
            mtime_only_mismatch,
            inode_changed,
            files_in_corpus,
            files_indexed,
            files_unindexed,
            files_missing,
            directories_missing,
            files_relocated,
            corrupt_files,
            shit_format_files,
            file_type_breakdown,
            _directory_breakdown: directory_breakdown,
        })
    }

    fn compute_library_deploy_bucket(&self) -> Result<crate::corpus::db::types::LibraryDeployBucket> {
        use crate::corpus::db::types::*;

        let library_stale = self.count_signal_type("library_stale")?;
        let library_leftover = self.count_signal_type("library_leftover")?;
        let deploy_ready = self.count_signal_type("deploy_ready")?;
        let deployed_healthy = self.count_signal_type("deployed_healthy")?;

        Ok(LibraryDeployBucket {
            library_stale,
            library_leftover,
            deploy_ready,
            deployed_healthy,
        })
    }

    fn compute_tag_resolution_bucket(&self) -> Result<crate::corpus::db::types::TagSquashBucket> {
        use crate::corpus::db::types::*;

        // Cross-source overlap clusters (easy resolutions - at top of bucket)
        // These are derived from fingerprint overlaps, clustered by source directory
        let directory_overlap_cluster_count = self.count_signal_type("cross_source_overlap")?;

        // Subpar duplicates (lower quality versions identified by fingerprint analysis)
        // Note: stored as "subpar_duplicate" in database for backwards compatibility
        let subpar_duplicate_count = self.count_signal_type("subpar_duplicate")?;

        // Count inconsistent_album_artist signals
        let inconsistent_album_artist_count = self.count_signal_type("inconsistent_album_artist")?;

        // Count compound_tag_value signals
        let compound_tag_value_count = self.count_signal_type("compound_tag_value")?;

        // Group tag_canonicity signals by tag name (extracted from issue_key prefix)
        // Key format: "{tag_name}:{normalized_key}" e.g., "artist:dragonforce"
        // ORDER BY total_tracks DESC - tags affecting more tracks should appear first
        let mut stmt = self.conn.prepare(
            r#"SELECT
                SUBSTR(issue_key, 1, INSTR(issue_key, ':') - 1) as tag_name,
                COUNT(*) as cluster_count,
                COALESCE(SUM(json_array_length(json_extract(metadata_json, '$.inodes'))), 0) as total_tracks
            FROM signals
            WHERE issue_type = 'tag_canonicity'
            GROUP BY tag_name
            ORDER BY total_tracks DESC"#
        )?;

        let tag_canonicity: Vec<TagSquashEntry> = stmt
            .query_map(params![], |row| {
                Ok(TagSquashEntry {
                    tag_name: row.get(0)?,
                    cluster_count: row.get(1)?,
                    _total_tracks: row.get::<_, i64>(2).unwrap_or(0) as usize,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(TagSquashBucket {
            directory_overlap_cluster_count,
            subpar_duplicate_count,
            tag_canonicity,
            inconsistent_album_artist_count,
            compound_tag_value_count,
        })
    }

    fn compute_other_signals_bucket(&self) -> Result<crate::corpus::db::types::OtherSignalsBucket> {
        use crate::corpus::db::types::*;

        let mut entries = Vec::new();

        // Aggregate signals with affected counts
        // Note: fingerprint_dup and subpar_duplicate are now in the TagSquash bucket
        for (signal_type, label) in [
            ("metadata_dup", "Metadata Duplicates"),
            ("duplicate_inode", "Duplicate Inodes"),
            ("missing_tag", "Missing Tags"),
            ("deploy_conflict", "Deploy Conflicts"),
        ] {
            let count = self.count_signal_type(signal_type)?;

            if count > 0 {
                let affected = self.count_affected_by_signal(signal_type)?;
                entries.push(OtherSignalEntry {
                    signal_type: signal_type.to_string(),
                    display_label: label.to_string(),
                    count,
                    affected_count: if affected > 0 { Some(affected) } else { None },
                });
            }
        }

        // Sort by count descending
        entries.sort_by(|a, b| b.count.cmp(&a.count));

        Ok(OtherSignalsBucket { entries })
    }

    /// Count signals of a specific type by string.
    fn count_signal_type(&self, signal_type: &str) -> Result<usize> {
        let count: usize = self.conn.query_row(
            "SELECT COUNT(*) FROM signals WHERE issue_type = ?1",
            params![signal_type],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Count tracks affected by aggregate signals (sum of inode_count in metadata).
    fn count_affected_by_signal(&self, signal_type: &str) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            r#"SELECT COALESCE(SUM(
                 json_extract(metadata_json, '$.inode_count')
               ), 0)
               FROM signals
               WHERE issue_type = ?1"#,
            params![signal_type],
            |row| row.get(0),
        ).unwrap_or(0);
        Ok(count as usize)
    }

    /// Get file type breakdown from audio files (files + audio_info).
    fn get_file_type_breakdown(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT a.file_type, COUNT(*) as cnt
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.source = 'corpus' AND f.is_dir = 0
               GROUP BY a.file_type
               ORDER BY cnt DESC"#
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get directory breakdown for a signal type.
    fn get_directory_breakdown(&self, signal_type: &str) -> Result<crate::corpus::db::types::DirectoryBreakdown> {
        use crate::corpus::db::types::*;

        // Extract parent directory from issue_key (file path) and count
        // Using SQLite's string manipulation to get directory
        let mut stmt = self.conn.prepare(
            r#"SELECT
                 CASE
                   WHEN instr(issue_key, '/') > 0
                   THEN substr(issue_key, 1, length(issue_key) - length(replace(issue_key, '/', '')) -
                        length(substr(issue_key, length(issue_key) - length(replace(issue_key, '/', '')) + 1)))
                   ELSE ''
                 END as dir,
                 COUNT(*) as cnt
               FROM signals
               WHERE issue_type = ?1
               GROUP BY dir
               ORDER BY cnt DESC
               LIMIT 50"#
        )?;

        let entries = stmt.query_map(params![signal_type], |row| {
            Ok(DirectoryBreakdownEntry {
                _directory: row.get(0)?,
                _count: row.get(1)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(DirectoryBreakdown { _entries: entries })
    }

    // ========================================================================
    // Row Conversion Helpers
    // ========================================================================

    /// Convert a row to Signal.
    /// Expected columns: id, issue_type, issue_key, discovered_at, metadata_json
    pub(super) fn row_to_signal(row: &rusqlite::Row) -> rusqlite::Result<Signal> {
        let issue_type_str: String = row.get(1)?;

        Ok(Signal {
            id: Some(row.get(0)?),
            issue_type: SignalType::from_str(&issue_type_str)
                .unwrap_or(SignalType::FingerprintOverlap),
            issue_key: row.get(2)?,
            discovered_at: row.get(3)?,
            metadata_json: row.get(4)?,
        })
    }

    // ========================================================================
    // Deploy Modal Queries
    // ========================================================================

    /// Get all deploy-ready files (healthy corpus files not yet in library).
    ///
    /// Returns files with their corpus path and computed deploy path.
    /// Sorted by corpus_path for consistent display.
    pub fn get_deploy_ready_files(&self) -> Result<Vec<crate::corpus::db::types::DeploySignalFile>> {
        use crate::corpus::db::types::DeploySignalFile;

        // deploy_ready signals: issue_key = corpus_path, metadata_json contains deploy_path
        // Join with files table to get inode (used as identifier for mutations)
        let mut stmt = self.conn.prepare(
            r#"SELECT
                 h.issue_key as corpus_path,
                 json_extract(h.metadata_json, '$.deploy_path') as deploy_path,
                 COALESCE(f.inode, 0) as inode
               FROM signals h
               LEFT JOIN files f ON f.path = h.issue_key AND f.source = 'corpus'
               WHERE h.issue_type = 'deploy_ready'
               ORDER BY h.issue_key"#
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(DeploySignalFile {
                corpus_path: row.get(0)?,
                deploy_path: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all deployed healthy files (corpus files correctly deployed to library).
    ///
    /// Returns files with their corpus path and library path.
    /// Sorted by corpus_path for consistent display.
    pub fn get_deployed_healthy_files(&self) -> Result<Vec<crate::corpus::db::types::DeploySignalFile>> {
        use crate::corpus::db::types::DeploySignalFile;

        // deployed_healthy signals: issue_key = corpus_path, metadata_json contains library_path
        let mut stmt = self.conn.prepare(
            r#"SELECT
                 h.issue_key as corpus_path,
                 json_extract(h.metadata_json, '$.library_path') as library_path
               FROM signals h
               WHERE h.issue_type = 'deployed_healthy'
               ORDER BY h.issue_key"#
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(DeploySignalFile {
                corpus_path: row.get(0)?,
                deploy_path: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all stale library files (deployed at wrong path due to tag changes).
    ///
    /// Returns files with their current library path and expected path.
    /// Sorted by library_path for consistent display.
    pub fn get_library_stale_files(&self) -> Result<Vec<crate::corpus::db::types::StaleSignalFile>> {
        use crate::corpus::db::types::StaleSignalFile;

        // library_stale signals: all fields stored in metadata_json
        let mut stmt = self.conn.prepare(
            r#"SELECT
                 json_extract(h.metadata_json, '$.library_path') as library_path,
                 json_extract(h.metadata_json, '$.expected_path') as expected_path
               FROM signals h
               WHERE h.issue_type = 'library_stale'
               ORDER BY json_extract(h.metadata_json, '$.library_path')"#
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(StaleSignalFile {
                library_path: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                expected_path: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all leftover library files (no corpus backing).
    ///
    /// Sorted by library_path for consistent display.
    pub fn get_library_leftover_files(&self) -> Result<Vec<crate::corpus::db::types::LeftoverSignalFile>> {
        use crate::corpus::db::types::LeftoverSignalFile;

        // library_leftover signals: issue_key = "library_leftover:{library_name}:{library_path}"
        // We need to extract just the library_path portion (after the second colon)
        let mut stmt = self.conn.prepare(
            r#"SELECT issue_key
               FROM signals
               WHERE issue_type = 'library_leftover'
               ORDER BY issue_key"#
        )?;

        let results = stmt.query_map(params![], |row| {
            let issue_key: String = row.get(0)?;
            // Extract library_path from "library_leftover:{library_name}:{library_path}"
            // Library paths start with '/', so find ":/" to locate the path portion
            let library_path = if let Some(path_start) = issue_key.find(":/") {
                issue_key[path_start + 1..].to_string()
            } else {
                issue_key // Fallback: return full key if format unexpected
            };
            Ok(LeftoverSignalFile { library_path })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all deploy conflict groups (multiple corpus files → same library path).
    ///
    /// Sorted by deploy_path for consistent display.
    pub fn get_deploy_conflict_groups(&self) -> Result<Vec<crate::corpus::db::types::ConflictGroup>> {
        use crate::corpus::db::types::ConflictGroup;

        // deploy_conflict signals: issue_key = deploy_path, metadata_json contains inodes
        let mut stmt = self.conn.prepare(
            r#"SELECT
                 h.issue_key as deploy_path,
                 h.metadata_json
               FROM signals h
               WHERE h.issue_type = 'deploy_conflict'
               ORDER BY h.issue_key"#
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let deploy_path: String = row.get(0)?;
            let metadata_json: Option<String> = row.get(1)?;
            Ok((deploy_path, metadata_json))
        })?;

        for row in rows {
            let (deploy_path, metadata_json) = row?;

            // Extract inodes from metadata
            let inodes: Vec<i64> = metadata_json
                .as_ref()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
                .and_then(|v| v.get("inodes").cloned())
                .and_then(|v| v.as_array().cloned())
                .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
                .unwrap_or_default();

            // Get corpus paths for each file
            // Deploy conflicts are between corpus files
            let mut conflicting_files = Vec::new();
            for inode in inodes {
                if let Ok(Some(audio_file)) = self.get_audio_file_by_inode(inode, FileSource::Corpus) {
                    conflicting_files.push((audio_file.path().to_string(), inode));
                }
            }

            results.push(ConflictGroup {
                deploy_path,
                conflicting_files,
            });
        }

        Ok(results)
    }

    // ========================================================================
    // Missing File Resolution Queries
    // ========================================================================

    /// Get all corpus paths with MissingFile signals.
    ///
    /// Returns the issue_key (corpus path) for each missing_file signal.
    /// Used by the missing file resolution modal to categorize files.
    pub fn get_missing_file_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT issue_key FROM signals WHERE issue_type = 'missing_file' ORDER BY issue_key"
        )?;

        let results = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        Ok(results)
    }

    // ========================================================================
    // Corrupt File Resolution Queries
    // ========================================================================

    /// Get all corpus paths with CorruptFile signals.
    ///
    /// Returns the issue_key (corpus path) for each corrupt_file signal.
    /// Used by the corrupt file resolution modal.
    pub fn get_corrupt_file_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT issue_key FROM signals WHERE issue_type = 'corrupt_file' ORDER BY issue_key"
        )?;

        let results = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        Ok(results)
    }

    // ========================================================================
    // Shit Format Resolution Queries
    // ========================================================================

    /// Get all corpus paths with ShitFormat signals.
    ///
    /// Returns (issue_key, file_type) for each shit_format signal.
    /// The file_type is extracted from metadata_json.
    /// Used by the shit format resolution modal.
    pub fn get_shit_format_files(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT issue_key, COALESCE(json_extract(metadata_json, '$.file_type'), '')
               FROM signals
               WHERE issue_type = 'shit_format'
               ORDER BY issue_key"#
        )?;

        let results = stmt
            .query_map(params![], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, String)>>>()?;

        Ok(results)
    }

    /// Get counts of shit format files grouped by file type.
    ///
    /// Returns (file_type, count) pairs sorted by count descending.
    pub fn get_shit_format_counts_by_type(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT COALESCE(json_extract(metadata_json, '$.file_type'), 'unknown') as file_type,
                      COUNT(*) as cnt
               FROM signals
               WHERE issue_type = 'shit_format'
               GROUP BY file_type
               ORDER BY cnt DESC"#
        )?;

        let results = stmt
            .query_map(params![], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<(String, i64)>>>()?;

        Ok(results)
    }

    // ========================================================================
    // Subpar Duplicate Resolution Queries
    // ========================================================================

    /// Get all subpar duplicate files with metadata.
    ///
    /// Returns (corpus_path, reason, superior_path) for each subpar_duplicate signal.
    /// Used by the subpar duplicate resolution modal.
    pub fn get_subpar_duplicate_files(&self) -> Result<Vec<crate::corpus::db::types::SubparDuplicateEntry>> {
        use crate::corpus::db::types::SubparDuplicateEntry;

        let mut stmt = self.conn.prepare(
            r#"SELECT
                 issue_key,
                 COALESCE(json_extract(metadata_json, '$.reason'), 'unknown') as reason,
                 COALESCE(json_extract(metadata_json, '$.superior_path'), '') as superior_path,
                 COALESCE(json_extract(metadata_json, '$.quality_score'), 0) as quality_score,
                 COALESCE(json_extract(metadata_json, '$.superior_quality_score'), 0) as superior_quality_score
               FROM signals
               WHERE issue_type = 'subpar_duplicate'
               ORDER BY issue_key"#
        )?;

        let results = stmt
            .query_map(params![], |row| {
                Ok(SubparDuplicateEntry {
                    corpus_path: row.get(0)?,
                    reason: row.get(1)?,
                    superior_path: row.get(2)?,
                    _quality_score: row.get(3)?,
                    _superior_quality_score: row.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<SubparDuplicateEntry>>>()?;

        Ok(results)
    }
}
