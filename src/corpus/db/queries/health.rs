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
use crate::corpus::db::types::FileSource;
use crate::meta::signals::{
    AggregateSignal, AggregateSignalType, CorpusFileSignalType,
    Signal, SignalType,
};

impl Database {
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
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json, inode
                   FROM signals
                   WHERE issue_type = ?1
                   ORDER BY discovered_at DESC"#
            }
            None => {
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json, inode
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
                r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json, inode
                   FROM signals
                   WHERE id = ?1"#,
                params![signal_id],
                Self::row_to_signal,
            )
            .optional()
            .context("Failed to query signal by ID")
    }

    /// Fast existence check for an aggregate signal (semantic-keyed).
    ///
    /// Used for LibraryStale, LibraryLeftover, and other semantic-keyed signals.
    pub fn aggregate_signal_exists(&self, signal_type: AggregateSignalType, key: &str) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM signals WHERE issue_type = ?1 AND issue_key = ?2 LIMIT 1",
                params![signal_type.as_str(), key],
                |_| Ok(()),
            )
            .is_ok()
    }

    // ========================================================================
    // Inode-Native Signal Operations
    // ========================================================================

    /// Fast existence check for an inode-keyed corpus signal.
    ///
    /// Uses the native `inode` column for efficient lookup.
    pub fn corpus_signal_exists_by_inode(
        &self,
        signal_type: CorpusFileSignalType,
        inode: i64,
    ) -> bool {
        self.conn
            .query_row(
                "SELECT 1 FROM signals WHERE issue_type = ?1 AND inode = ?2 LIMIT 1",
                params![signal_type.as_str(), inode],
                |_| Ok(()),
            )
            .is_ok()
    }

    /// Ensure an inode-keyed corpus signal exists (idempotent).
    ///
    /// Uses the native `inode` column. The path is stored in `metadata_json`
    /// for display purposes, and `issue_key` is set to the inode as string
    /// for backwards compatibility.
    pub fn ensure_corpus_signal(
        &self,
        signal_type: CorpusFileSignalType,
        inode: i64,
        path: &str,
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        let key = inode.to_string();
        let metadata = serde_json::json!({ "path": path });
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO signals
                (issue_type, issue_key, inode, discovered_at, metadata_json)
                VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, ?4)
                "#,
                params![signal_type.as_str(), key, inode, metadata.to_string()],
            )
            .context("Failed to ensure corpus signal")?;

        Ok(self.conn.changes() > 0)
    }

    /// Ensure an inode-keyed corpus signal with additional metadata.
    ///
    /// Merges the path into the provided metadata and stores the signal.
    pub fn ensure_corpus_signal_with_metadata(
        &self,
        signal_type: CorpusFileSignalType,
        inode: i64,
        path: &str,
        mut extra_metadata: serde_json::Value,
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        let key = inode.to_string();
        extra_metadata["path"] = serde_json::json!(path);
        self.conn
            .execute(
                r#"
                INSERT OR IGNORE INTO signals
                (issue_type, issue_key, inode, discovered_at, metadata_json)
                VALUES (?1, ?2, ?3, CURRENT_TIMESTAMP, ?4)
                "#,
                params![signal_type.as_str(), key, inode, extra_metadata.to_string()],
            )
            .context("Failed to ensure corpus signal with metadata")?;

        Ok(self.conn.changes() > 0)
    }

    /// Clear an inode-keyed corpus signal (idempotent delete).
    pub fn clear_corpus_signal(
        &self,
        signal_type: CorpusFileSignalType,
        inode: i64,
        _witness: &impl SignalWitness,
    ) -> Result<bool> {
        let deleted = self.conn
            .execute(
                "DELETE FROM signals WHERE issue_type = ?1 AND inode = ?2",
                params![signal_type.as_str(), inode],
            )
            .context("Failed to clear corpus signal")?;

        Ok(deleted > 0)
    }

    /// Clear all corpus signals for an inode.
    ///
    /// Used when dropping a file from the index to clear all associated signals.
    pub fn clear_all_corpus_signals_for_inode(
        &self,
        inode: i64,
        _witness: &impl SignalWitness,
    ) -> Result<usize> {
        let deleted = self.conn
            .execute(
                "DELETE FROM signals WHERE inode = ?1",
                params![inode],
            )
            .context("Failed to clear corpus signals for inode")?;

        Ok(deleted)
    }

    /// Delete all signals for a specific path (for path-keyed signals like library signals).
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
    // Type-Safe Signal Operations (Aggregate)
    // ========================================================================
    // NOTE: ensure_file_signal/clear_file_signal have been removed.
    // - Corpus signals: use ensure_corpus_signal/clear_corpus_signal (inode-keyed)
    // - Library signals: use ensure_aggregate_signal/clear_aggregate_signal (semantic-keyed)

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

    /// Get compound tag signals filtered by safety classification.
    ///
    /// If `safe_only` is true, returns only signals where ALL compounds have
    /// all split parts existing in corpus (matching_parts.len() == split_parts.len()).
    /// If false, returns only signals that need review (some/all parts are new).
    /// If `tag_filter` is Some, only returns signals containing compounds for that tag name.
    pub fn get_compound_signals_by_safety(&self, safe_only: bool, tag_filter: Option<&str>) -> Result<Vec<AggregateSignal>> {
        // Note: Uses 'compound_tag' (per-file signal) not 'compound_tag_value' (aggregate)
        // The per-file CompoundTag signals are emitted by DetectCompoundTagsForInode
        let mut stmt = self.conn.prepare(
            r#"SELECT id, issue_type, issue_key, discovered_at, metadata_json
               FROM signals WHERE issue_type = 'compound_tag'
               ORDER BY discovered_at DESC"#,
        )?;

        let row_mapper = |row: &rusqlite::Row| {
            let id: i64 = row.get(0)?;
            let type_str: String = row.get(1)?;
            let key: String = row.get(2)?;
            let discovered_at: Option<String> = row.get(3)?;
            let metadata_json: Option<String> = row.get(4)?;

            let signal_type = AggregateSignalType::from_str(&type_str)
                .unwrap_or(AggregateSignalType::CompoundTagValue);

            Ok(AggregateSignal {
                id: Some(id),
                signal_type,
                key,
                discovered_at,
                metadata_json,
            })
        };

        let rows = stmt.query_map(params![], row_mapper)?;

        let mut results = Vec::new();
        for row in rows {
            let signal = row?;

            // Parse and classify this signal
            let dominated_tag_name;
            let is_safe = match &signal.metadata_json {
                Some(metadata) => {
                    match serde_json::from_str::<serde_json::Value>(metadata) {
                        Ok(json) => {
                            let compounds = json.get("compounds").and_then(|v| v.as_array());
                            match compounds {
                                Some(arr) if !arr.is_empty() => {
                                    // Extract the tag name from the first compound
                                    dominated_tag_name = arr.first()
                                        .and_then(|c| c.get("tag_name"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| s.to_string());

                                    arr.iter().all(|compound| {
                                        let split_len = compound
                                            .get("split_parts")
                                            .and_then(|v| v.as_array())
                                            .map(|a| a.len())
                                            .unwrap_or(0);
                                        let match_len = compound
                                            .get("matching_parts")
                                            .and_then(|v| v.as_array())
                                            .map(|a| a.len())
                                            .unwrap_or(0);
                                        split_len > 0 && split_len == match_len
                                    })
                                }
                                _ => {
                                    dominated_tag_name = None;
                                    false
                                }
                            }
                        }
                        Err(_) => {
                            dominated_tag_name = None;
                            false
                        }
                    }
                }
                None => {
                    dominated_tag_name = None;
                    false
                }
            };

            // Check safety classification
            if is_safe != safe_only {
                continue;
            }

            // Check tag filter if specified
            if let Some(filter) = tag_filter {
                match &dominated_tag_name {
                    Some(tag) if tag == filter => {}
                    _ => continue,
                }
            }

            results.push(signal);
        }

        Ok(results)
    }

    // ========================================================================
    // Directory-Level Queries
    // ========================================================================

    /// Get indexed corpus directories with their inodes.
    ///
    /// Returns (path, inode) tuples from the files table where is_dir=1 and source='corpus'.
    /// Used to detect missing directories (directories that were indexed but no longer exist).
    pub fn get_indexed_corpus_directories(&self) -> Result<Vec<(std::path::PathBuf, i64)>> {
        use std::path::PathBuf;

        let mut stmt = self.conn.prepare(
            r#"SELECT path, inode FROM files
               WHERE is_dir = 1 AND source = 'corpus'"#
        )?;
        let rows = stmt.query_map(params![], |row| {
            let path: String = row.get(0)?;
            let inode: i64 = row.get(1)?;
            Ok((PathBuf::from(path), inode))
        })?;

        let mut directories = Vec::new();
        for row in rows {
            directories.push(row?);
        }

        Ok(directories)
    }

    /// Get missing directory signal paths (for UI resolution modal).
    ///
    /// The path is stored in metadata_json (issue_key contains the inode).
    pub fn get_missing_directory_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT json_extract(metadata_json, '$.path') FROM signals WHERE issue_type = 'missing_directory' ORDER BY json_extract(metadata_json, '$.path')"
        )?;
        let rows = stmt.query_map(params![], |row| row.get::<_, Option<String>>(0))?;

        let mut paths = Vec::new();
        for row in rows {
            if let Some(path) = row? {
                paths.push(path);
            }
        }

        Ok(paths)
    }

    /// Get all FileInCorpus signal inodes with their paths.
    ///
    /// FileInCorpus signals are keyed by inode (stored as string in issue_key)
    /// with the path stored in metadata_json.path.
    ///
    /// Returns HashMap<inode, path> for set comparison operations.
    pub fn get_file_in_corpus_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT issue_key, metadata_json FROM signals
               WHERE issue_type = 'file_in_corpus'"#
        )?;

        let rows = stmt.query_map(params![], |row| {
            let key: String = row.get(0)?;
            let metadata: Option<String> = row.get(1)?;
            Ok((key, metadata))
        })?;

        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (key, metadata) = row?;
            // Parse inode from issue_key
            if let Ok(inode) = key.parse::<i64>() {
                // Extract path from metadata_json
                let path = metadata
                    .and_then(|m| serde_json::from_str::<serde_json::Value>(&m).ok())
                    .and_then(|v| v.get("path").and_then(|p| p.as_str()).map(String::from))
                    .unwrap_or_default();
                result.insert(inode, path);
            }
        }

        Ok(result)
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

        // Group compound_tag signals by tag name with safety classification
        let compound_tags = self.count_compound_signals_by_tag()?;

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
            compound_tags,
        })
    }

    /// Count compound tag signals by safety classification, grouped by tag name.
    ///
    /// A compound signal is "safe" if ALL its compounds have all split parts
    /// existing in the corpus (matching_parts.len() == split_parts.len()).
    /// Returns entries grouped by tag name, sorted by total count descending.
    fn count_compound_signals_by_tag(&self) -> Result<Vec<crate::corpus::db::types::CompoundTagEntry>> {
        use std::collections::HashMap;
        use crate::corpus::db::types::CompoundTagEntry;

        // Note: Uses 'compound_tag' (per-file signal) not 'compound_tag_value' (aggregate)
        // The per-file CompoundTag signals are emitted by DetectCompoundTagsForInode
        let mut stmt = self.conn.prepare(
            r#"SELECT metadata_json FROM signals WHERE issue_type = 'compound_tag'"#,
        )?;

        // Map: tag_name -> (safe_count, review_count)
        let mut by_tag: HashMap<String, (usize, usize)> = HashMap::new();

        let rows = stmt.query_map(params![], |row| {
            let metadata: Option<String> = row.get(0)?;
            Ok(metadata)
        })?;

        for row in rows {
            let metadata = match row? {
                Some(m) => m,
                None => continue, // Skip signals without metadata
            };

            let json: serde_json::Value = match serde_json::from_str(&metadata) {
                Ok(v) => v,
                Err(_) => continue, // Skip malformed JSON
            };

            // Process each compound in the signal
            let compounds = json.get("compounds").and_then(|v| v.as_array());
            if let Some(arr) = compounds {
                for compound in arr {
                    let tag_name = compound
                        .get("tag_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    let split_len = compound
                        .get("split_parts")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);
                    let match_len = compound
                        .get("matching_parts")
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);

                    let is_safe = split_len > 0 && split_len == match_len;

                    let entry = by_tag.entry(tag_name).or_insert((0, 0));
                    if is_safe {
                        entry.0 += 1;
                    } else {
                        entry.1 += 1;
                    }
                }
            }
        }

        // Convert to vec and sort by total count descending
        let mut entries: Vec<CompoundTagEntry> = by_tag
            .into_iter()
            .map(|(tag_name, (safe_count, review_count))| CompoundTagEntry {
                tag_name,
                safe_count,
                review_count,
            })
            .collect();

        entries.sort_by(|a, b| {
            let total_a = a.safe_count + a.review_count;
            let total_b = b.safe_count + b.review_count;
            total_b.cmp(&total_a)
        });

        Ok(entries)
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
    /// Expected columns: id, issue_type, issue_key, discovered_at, metadata_json, inode
    pub(super) fn row_to_signal(row: &rusqlite::Row) -> rusqlite::Result<Signal> {
        let issue_type_str: String = row.get(1)?;

        Ok(Signal {
            id: Some(row.get(0)?),
            issue_type: SignalType::from_str(&issue_type_str)
                .unwrap_or(SignalType::FingerprintOverlap),
            issue_key: row.get(2)?,
            discovered_at: row.get(3)?,
            metadata_json: row.get(4)?,
            inode: row.get(5)?,
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

        // deploy_ready signals are inode-keyed: issue_key = inode (as string)
        // Path is in metadata_json.path, deploy_path is in metadata_json.deploy_path
        let mut stmt = self.conn.prepare(
            r#"SELECT
                 json_extract(h.metadata_json, '$.path') as corpus_path,
                 json_extract(h.metadata_json, '$.deploy_path') as deploy_path
               FROM signals h
               WHERE h.issue_type = 'deploy_ready'
               ORDER BY json_extract(h.metadata_json, '$.path')"#
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(DeploySignalFile {
                corpus_path: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
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

        // deployed_healthy signals are inode-keyed: issue_key = inode (as string)
        // Path is in metadata_json.path, library_path is in metadata_json.library_path
        let mut stmt = self.conn.prepare(
            r#"SELECT
                 json_extract(h.metadata_json, '$.path') as corpus_path,
                 json_extract(h.metadata_json, '$.library_path') as library_path
               FROM signals h
               WHERE h.issue_type = 'deployed_healthy'
               ORDER BY json_extract(h.metadata_json, '$.path')"#
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(DeploySignalFile {
                corpus_path: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
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
    /// Returns the path from metadata_json for each missing_file signal.
    /// Used by the missing file resolution modal to categorize files.
    /// MissingFile signals are keyed by inode with path in metadata.
    pub fn get_missing_file_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT COALESCE(json_extract(metadata_json, '$.path'), '')
               FROM signals WHERE issue_type = 'missing_file'
               ORDER BY json_extract(metadata_json, '$.path')"#
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
    /// Returns the path from metadata_json for each corrupt_file signal.
    /// Used by the corrupt file resolution modal.
    /// CorruptFile signals are keyed by inode with path in metadata.
    pub fn get_corrupt_file_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT COALESCE(json_extract(metadata_json, '$.path'), '')
               FROM signals WHERE issue_type = 'corrupt_file'
               ORDER BY json_extract(metadata_json, '$.path')"#
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
    /// Returns (path, file_type) for each shit_format signal.
    /// Both values are extracted from metadata_json.
    /// Used by the shit format resolution modal.
    /// ShitFormat signals are keyed by inode with path in metadata.
    pub fn get_shit_format_files(&self) -> Result<Vec<(String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT COALESCE(json_extract(metadata_json, '$.path'), ''),
                      COALESCE(json_extract(metadata_json, '$.file_type'), '')
               FROM signals
               WHERE issue_type = 'shit_format'
               ORDER BY json_extract(metadata_json, '$.path')"#
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

    // ========================================================================
    // CanonicalTag Whitelist Queries
    // ========================================================================

    /// Check if a CanonicalTag signal exists for this tag_name:tag_value.
    ///
    /// Used to skip compound tag detection for operator-confirmed canonical values.
    /// For example, if "artist:Rinse & Repeat" is marked canonical, we shouldn't
    /// flag it for splitting even though it contains " & ".
    pub fn is_canonical_tag(&self, tag_name: &str, tag_value: &str) -> Result<bool> {
        let key = format!("{}:{}", tag_name, tag_value);
        let exists: bool = self.conn
            .query_row(
                "SELECT 1 FROM signals WHERE issue_type = 'canonical_tag' AND issue_key = ?1 LIMIT 1",
                params![key],
                |_| Ok(true),
            )
            .unwrap_or(false);
        Ok(exists)
    }
}
