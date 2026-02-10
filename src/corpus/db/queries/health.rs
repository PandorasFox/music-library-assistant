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
use rusqlite::params;

use super::Database;
use crate::db_thread::SignalWitness;
use crate::corpus::db::types::FileSource;
use crate::meta::signals::{
    AggregateSignal, AggregateSignalType, CorpusFileSignalType,
};

impl Database {
    // ========================================================================
    // Health Issue Operations
    // ========================================================================


    /// Count total signals across all typed tables.
    ///
    /// More efficient than loading all signals into memory with get_signals(None).
    pub fn count_all_signals(&self) -> usize {
        use crate::meta::signals::data::*;
        use crate::meta::signals::store::{CorpusSignalStore, AggregateSignalStore};

        let mut total: usize = 0;
        // Corpus signal tables
        total += FileInCorpusSignal::count(&self.conn).unwrap_or(0) as usize;
        total += UnindexedFileSignal::count(&self.conn).unwrap_or(0) as usize;
        total += HealthyFileSignal::count(&self.conn).unwrap_or(0) as usize;
        total += CorruptFileSignal::count(&self.conn).unwrap_or(0) as usize;
        total += MtimeOnlyMismatchSignal::count(&self.conn).unwrap_or(0) as usize;
        total += MissingDirectorySignal::count(&self.conn).unwrap_or(0) as usize;
        total += MissingFileSignal::count(&self.conn).unwrap_or(0) as usize;
        total += MovedFileSignal::count(&self.conn).unwrap_or(0) as usize;
        total += ShitFormatSignal::count(&self.conn).unwrap_or(0) as usize;
        total += DeployReadySignal::count(&self.conn).unwrap_or(0) as usize;
        total += DeployedHealthySignal::count(&self.conn).unwrap_or(0) as usize;
        total += OutOfBandTagSyncSignal::count(&self.conn).unwrap_or(0) as usize;
        total += OutOfBandTagConflictSignal::count(&self.conn).unwrap_or(0) as usize;
        total += SubparDuplicateSignal::count(&self.conn).unwrap_or(0) as usize;
        total += CompoundTagSignal::count(&self.conn).unwrap_or(0) as usize;
        // Aggregate signal tables
        total += FingerprintOverlapSignal::count(&self.conn).unwrap_or(0) as usize;
        total += MetadataDuplicateSignal::count(&self.conn).unwrap_or(0) as usize;
        total += DuplicateInodeSignal::count(&self.conn).unwrap_or(0) as usize;
        total += MissingTagSignal::count(&self.conn).unwrap_or(0) as usize;
        total += DeployConflictSignal::count(&self.conn).unwrap_or(0) as usize;
        total += TagCanonicitySignal::count(&self.conn).unwrap_or(0) as usize;
        total += InconsistentAlbumArtistSignal::count(&self.conn).unwrap_or(0) as usize;
        total += CompoundTagValueSignal::count(&self.conn).unwrap_or(0) as usize;
        total += CrossSourceOverlapSignal::count(&self.conn).unwrap_or(0) as usize;
        total += CanonicalTagSignal::count(&self.conn).unwrap_or(0) as usize;
        total += LibraryLeftoverSignal::count(&self.conn).unwrap_or(0) as usize;
        total += LibraryStaleSignal::count(&self.conn).unwrap_or(0) as usize;
        total
    }

    // ========================================================================
    // Typed Signal Queries (direct struct access, no JSON)
    // ========================================================================

    pub fn get_unindexed_file_signals(&self) -> Result<Vec<crate::meta::signals::data::UnindexedFileSignal>> {
        crate::meta::signals::data::UnindexedFileSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query unindexed file signals: {}", e))
    }

    pub fn get_healthy_file_signals(&self) -> Result<Vec<crate::meta::signals::data::HealthyFileSignal>> {
        crate::meta::signals::data::HealthyFileSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query healthy file signals: {}", e))
    }

    pub fn get_tag_canonicity_signal(&self, key: &str) -> Result<Option<crate::meta::signals::data::TagCanonicitySignal>> {
        crate::meta::signals::data::TagCanonicitySignal::query_by_key(&self.conn, key)
            .map_err(|e| anyhow::anyhow!("Failed to query tag canonicity signal: {}", e))
    }

    pub fn get_inconsistent_album_artist_signal(&self, key: &str) -> Result<Option<crate::meta::signals::data::InconsistentAlbumArtistSignal>> {
        crate::meta::signals::data::InconsistentAlbumArtistSignal::query_by_key(&self.conn, key)
            .map_err(|e| anyhow::anyhow!("Failed to query inconsistent album artist signal: {}", e))
    }

    pub fn get_compound_tag_signal(&self, inode: i64) -> Result<Option<crate::meta::signals::data::CompoundTagSignal>> {
        crate::meta::signals::data::CompoundTagSignal::query_by_inode(&self.conn, inode)
            .map_err(|e| anyhow::anyhow!("Failed to query compound tag signal: {}", e))
    }

    pub fn get_cross_source_overlap_signals(&self) -> Result<Vec<crate::meta::signals::data::CrossSourceOverlapSignal>> {
        crate::meta::signals::data::CrossSourceOverlapSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query cross source overlap signals: {}", e))
    }

    /// Fast existence check for an aggregate signal (semantic-keyed).
    ///
    /// Queries the per-signal typed table directly.
    pub fn aggregate_signal_exists(&self, signal_type: AggregateSignalType, key: &str) -> bool {
        use crate::meta::signals::data::*;
        use crate::meta::signals::store::AggregateSignalStore;
        match signal_type {
            AggregateSignalType::FingerprintOverlap => FingerprintOverlapSignal::exists(&self.conn, key),
            AggregateSignalType::MetadataDuplicate => MetadataDuplicateSignal::exists(&self.conn, key),
            AggregateSignalType::DuplicateInode => DuplicateInodeSignal::exists(&self.conn, key),
            AggregateSignalType::MissingTag => MissingTagSignal::exists(&self.conn, key),
            AggregateSignalType::DeployConflict => DeployConflictSignal::exists(&self.conn, key),
            AggregateSignalType::TagCanonicity => TagCanonicitySignal::exists(&self.conn, key),
            AggregateSignalType::InconsistentAlbumArtist => InconsistentAlbumArtistSignal::exists(&self.conn, key),
            AggregateSignalType::CompoundTagValue => CompoundTagValueSignal::exists(&self.conn, key),
            AggregateSignalType::CrossSourceOverlap => CrossSourceOverlapSignal::exists(&self.conn, key),
            AggregateSignalType::CanonicalTag => CanonicalTagSignal::exists(&self.conn, key),
            AggregateSignalType::LibraryLeftover => LibraryLeftoverSignal::exists(&self.conn, key),
            AggregateSignalType::LibraryStale => LibraryStaleSignal::exists(&self.conn, key),
        }.unwrap_or(false)
    }

    // ========================================================================
    // Inode-Native Signal Operations
    // ========================================================================

    /// Fast existence check for an inode-keyed corpus signal.
    ///
    /// Queries the per-signal typed table directly.
    pub fn corpus_signal_exists_by_inode(
        &self,
        signal_type: CorpusFileSignalType,
        inode: i64,
    ) -> bool {
        use crate::meta::signals::data::*;
        use crate::meta::signals::store::CorpusSignalStore;
        match signal_type {
            CorpusFileSignalType::FileInCorpus => FileInCorpusSignal::exists(&self.conn, inode),
            CorpusFileSignalType::UnindexedFile => UnindexedFileSignal::exists(&self.conn, inode),
            CorpusFileSignalType::HealthyFile => HealthyFileSignal::exists(&self.conn, inode),
            CorpusFileSignalType::MissingFile => MissingFileSignal::exists(&self.conn, inode),
            CorpusFileSignalType::MissingDirectory => MissingDirectorySignal::exists(&self.conn, inode),
            CorpusFileSignalType::MovedFile => MovedFileSignal::exists(&self.conn, inode),
            CorpusFileSignalType::OutOfBandTagSync => OutOfBandTagSyncSignal::exists(&self.conn, inode),
            CorpusFileSignalType::OutOfBandTagConflict => OutOfBandTagConflictSignal::exists(&self.conn, inode),
            CorpusFileSignalType::MtimeOnlyMismatch => MtimeOnlyMismatchSignal::exists(&self.conn, inode),
            CorpusFileSignalType::CorruptFile => CorruptFileSignal::exists(&self.conn, inode),
            CorpusFileSignalType::ShitFormat => ShitFormatSignal::exists(&self.conn, inode),
            CorpusFileSignalType::SubparDuplicate => SubparDuplicateSignal::exists(&self.conn, inode),
            CorpusFileSignalType::CompoundTag => CompoundTagSignal::exists(&self.conn, inode),
            CorpusFileSignalType::DeployReady => DeployReadySignal::exists(&self.conn, inode),
            CorpusFileSignalType::DeployedHealthy => DeployedHealthySignal::exists(&self.conn, inode),
        }.unwrap_or(false)
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
        // Read from typed table, reconstruct metadata JSON for callers
        let table = aggregate_signal_table_name(signal_type);
        let has_data = aggregate_signal_has_data_blob(signal_type);

        if has_data {
            let sql = format!("SELECT key, data FROM {}", table);
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![], |row| {
                let key: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((key, blob))
            })?;

            let mut results = Vec::new();
            for row in rows {
                let (key, blob) = row?;
                // Reconstruct JSON from bincode for backwards compat
                let json = reconstruct_aggregate_metadata_json(signal_type, &key, &blob);
                results.push((key, Some(json)));
            }
            Ok(results)
        } else {
            let sql = format!("SELECT key FROM {}", table);
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![], |row| {
                let key: String = row.get(0)?;
                Ok((key, None))
            })?;

            let mut results = Vec::new();
            for row in rows {
                results.push(row?);
            }
            Ok(results)
        }
    }

    /// Get all aggregate signals of a given type.
    ///
    /// Reads from the per-signal typed table and reconstructs metadata_json
    /// from bincode for backwards compatibility.
    pub fn get_aggregate_signals(
        &self,
        signal_type: Option<AggregateSignalType>,
    ) -> Result<Vec<AggregateSignal>> {
        match signal_type {
            Some(agg_type) => self.get_aggregate_signals_from_typed_table(agg_type),
            None => Ok(Vec::new()),
        }
    }

    /// Read aggregate signals from the per-signal typed table.
    fn get_aggregate_signals_from_typed_table(
        &self,
        agg_type: AggregateSignalType,
    ) -> Result<Vec<AggregateSignal>> {
        let table = aggregate_signal_table_name(agg_type);
        let has_blob = aggregate_signal_has_data_blob(agg_type);

        if has_blob {
            let sql = format!(
                "SELECT key, data, discovered_at FROM {} ORDER BY discovered_at DESC",
                table
            );
            let mut stmt = self.conn.prepare(&sql)?;
            let rows = stmt.query_map(params![], |row| {
                let key: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                let discovered_at: Option<String> = row.get(2)?;
                Ok((key, blob, discovered_at))
            })?;
            let mut results = Vec::new();
            for row in rows {
                let (key, blob, discovered_at) = row?;
                let metadata_json = Some(reconstruct_aggregate_metadata_json(agg_type, &key, &blob));
                results.push(AggregateSignal {
                    id: None,
                    signal_type: agg_type,
                    key,
                    discovered_at,
                    metadata_json,
                });
            }
            Ok(results)
        } else {
            // No BLOB — simple key + discovered_at
            // For CanonicalTag: also has tag_name, canonical_value columns
            // For LibraryStale: has library_path, expected_path, corpus_path, inode columns
            // For LibraryLeftover: just key
            let sql = match agg_type {
                AggregateSignalType::CanonicalTag => {
                    format!("SELECT key, tag_name, canonical_value, discovered_at FROM {} ORDER BY discovered_at DESC", table)
                }
                AggregateSignalType::LibraryStale => {
                    format!("SELECT key, library_path, expected_path, corpus_path, inode, discovered_at FROM {} ORDER BY discovered_at DESC", table)
                }
                _ => {
                    format!("SELECT key, discovered_at FROM {} ORDER BY discovered_at DESC", table)
                }
            };
            let mut stmt = self.conn.prepare(&sql)?;
            let mut results = Vec::new();

            match agg_type {
                AggregateSignalType::CanonicalTag => {
                    let rows = stmt.query_map(params![], |row| {
                        let key: String = row.get(0)?;
                        let tag_name: String = row.get(1)?;
                        let canonical_value: String = row.get(2)?;
                        let discovered_at: Option<String> = row.get(3)?;
                        Ok(AggregateSignal {
                            id: None,
                            signal_type: agg_type,
                            key,
                            discovered_at,
                            metadata_json: Some(
                                serde_json::json!({"tag_name": tag_name, "canonical_value": canonical_value}).to_string()
                            ),
                        })
                    })?;
                    for row in rows { results.push(row?); }
                }
                AggregateSignalType::LibraryStale => {
                    let rows = stmt.query_map(params![], |row| {
                        let key: String = row.get(0)?;
                        let library_path: String = row.get(1)?;
                        let expected_path: String = row.get(2)?;
                        let corpus_path: String = row.get(3)?;
                        let inode: i64 = row.get(4)?;
                        let discovered_at: Option<String> = row.get(5)?;
                        Ok(AggregateSignal {
                            id: None,
                            signal_type: agg_type,
                            key,
                            discovered_at,
                            metadata_json: Some(
                                serde_json::json!({
                                    "library_path": library_path,
                                    "expected_path": expected_path,
                                    "corpus_path": corpus_path,
                                    "inode": inode
                                }).to_string()
                            ),
                        })
                    })?;
                    for row in rows { results.push(row?); }
                }
                _ => {
                    // LibraryLeftover — just key, no metadata
                    let rows = stmt.query_map(params![], |row| {
                        let key: String = row.get(0)?;
                        let discovered_at: Option<String> = row.get(1)?;
                        Ok(AggregateSignal {
                            id: None,
                            signal_type: agg_type,
                            key,
                            discovered_at,
                            metadata_json: None,
                        })
                    })?;
                    for row in rows { results.push(row?); }
                }
            }
            Ok(results)
        }
    }

    /// Get compound tag signals filtered by safety classification.
    ///
    /// If `safe_only` is true, returns only signals where ALL compounds have
    /// all split parts existing in corpus (matching_parts.len() == split_parts.len()).
    /// If false, returns only signals that need review (some/all parts are new).
    /// If `tag_filter` is Some, only returns signals containing compounds for that tag name.
    pub fn get_compound_signals_by_safety(&self, safe_only: bool, tag_filter: Option<&str>) -> Result<Vec<AggregateSignal>> {
        use crate::meta::signals::data::CompoundTagEntry as TypedEntry;

        let mut stmt = self.conn.prepare(
            "SELECT inode, path, data, discovered_at FROM signal_compound_tag ORDER BY discovered_at DESC"
        )?;

        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            let discovered_at: Option<String> = row.get(3)?;
            Ok((inode, path, blob, discovered_at))
        })?;

        let mut results = Vec::new();
        for row in rows {
            let (inode, path, blob, discovered_at) = row?;

            let compounds: Vec<TypedEntry> = match bincode::deserialize(&blob) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if compounds.is_empty() {
                continue;
            }

            let dominated_tag_name = Some(compounds[0].tag_name.clone());
            let is_safe = compounds.iter().all(|c| {
                !c.split_parts.is_empty() && c.split_parts.len() == c.matching_parts.len()
            });

            if is_safe != safe_only {
                continue;
            }

            if let Some(filter) = tag_filter {
                match &dominated_tag_name {
                    Some(tag) if tag == filter => {}
                    _ => continue,
                }
            }

            // Reconstruct AggregateSignal with JSON metadata for backwards compat with UI code
            let metadata = serde_json::json!({
                "path": path,
                "compounds": compounds.iter().map(|c| serde_json::json!({
                    "tag_name": c.tag_name,
                    "compound_value": c.compound_value,
                    "split_parts": c.split_parts,
                    "separator": c.separator,
                    "matching_parts": c.matching_parts,
                })).collect::<Vec<_>>()
            });

            results.push(AggregateSignal {
                id: Some(inode), // Use inode as ID for backwards compat
                signal_type: AggregateSignalType::CompoundTagValue,
                key: inode.to_string(),
                discovered_at,
                metadata_json: Some(metadata.to_string()),
            });
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
    pub fn get_missing_directory_paths(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT path FROM signal_missing_directory ORDER BY path"
        )?;
        let results = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(results)
    }

    /// Get all FileInCorpus signal inodes with their paths.
    ///
    /// Returns HashMap<inode, path> for set comparison operations.
    pub fn get_file_in_corpus_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT inode, path FROM signal_file_in_corpus"
        )?;

        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            Ok((inode, path))
        })?;

        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (inode, path) = row?;
            result.insert(inode, path);
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

        // Group tag_canonicity signals by tag_name column
        // Sum inodes from bincode BLOB data
        let mut tag_map: std::collections::HashMap<String, (usize, usize)> = std::collections::HashMap::new();
        {
            let mut stmt = self.conn.prepare(
                "SELECT tag_name, data FROM signal_tag_canonicity"
            )?;
            let rows = stmt.query_map(params![], |row| {
                let tag_name: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((tag_name, blob))
            })?;
            for row in rows {
                if let Ok((tag_name, blob)) = row {
                    let inode_count = bincode::deserialize::<crate::meta::signals::data::TagCanonicityData>(&blob)
                        .map(|d| d.inodes.len())
                        .unwrap_or(0);
                    let entry = tag_map.entry(tag_name).or_insert((0, 0));
                    entry.0 += 1; // cluster_count
                    entry.1 += inode_count; // total_tracks
                }
            }
        }
        // Drop the unused stmt (was a false start)
        let mut tag_canonicity: Vec<TagSquashEntry> = tag_map
            .into_iter()
            .map(|(tag_name, (cluster_count, total_tracks))| TagSquashEntry {
                tag_name,
                cluster_count,
                _total_tracks: total_tracks,
            })
            .collect();
        tag_canonicity.sort_by(|a, b| b._total_tracks.cmp(&a._total_tracks));

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
        use crate::meta::signals::data::CompoundTagEntry as TypedEntry;

        let mut stmt = self.conn.prepare(
            "SELECT data FROM signal_compound_tag"
        )?;

        // Map: tag_name -> (safe_count, review_count)
        let mut by_tag: HashMap<String, (usize, usize)> = HashMap::new();

        let rows = stmt.query_map(params![], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        })?;

        for row in rows {
            let blob = match row {
                Ok(b) => b,
                Err(_) => continue,
            };

            let compounds: Vec<TypedEntry> = match bincode::deserialize(&blob) {
                Ok(c) => c,
                Err(_) => continue,
            };

            for compound in &compounds {
                let is_safe = !compound.split_parts.is_empty()
                    && compound.split_parts.len() == compound.matching_parts.len();

                let entry = by_tag.entry(compound.tag_name.clone()).or_insert((0, 0));
                if is_safe {
                    entry.0 += 1;
                } else {
                    entry.1 += 1;
                }
            }
        }

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

    /// Count signals of a specific type using typed tables.
    fn count_signal_type(&self, signal_type: &str) -> Result<usize> {
        use crate::meta::signals::data::*;
        use crate::meta::signals::store::{CorpusSignalStore, AggregateSignalStore};
        let count = match signal_type {
            "file_in_corpus" => FileInCorpusSignal::count(&self.conn)?,
            "unindexed_file" => UnindexedFileSignal::count(&self.conn)?,
            "healthy_file" => HealthyFileSignal::count(&self.conn)?,
            "missing_file" => MissingFileSignal::count(&self.conn)?,
            "missing_directory" => MissingDirectorySignal::count(&self.conn)?,
            "moved_file" => MovedFileSignal::count(&self.conn)?,
            "oob_tag_sync" => OutOfBandTagSyncSignal::count(&self.conn)?,
            "oob_tag_conflict" | "oob_tag" => OutOfBandTagConflictSignal::count(&self.conn)?,
            "mtime_only_mismatch" => MtimeOnlyMismatchSignal::count(&self.conn)?,
            "corrupt_file" => CorruptFileSignal::count(&self.conn)?,
            "shit_format" => ShitFormatSignal::count(&self.conn)?,
            "subpar_duplicate" => SubparDuplicateSignal::count(&self.conn)?,
            "compound_tag" => CompoundTagSignal::count(&self.conn)?,
            "deploy_ready" => DeployReadySignal::count(&self.conn)?,
            "deployed_healthy" => DeployedHealthySignal::count(&self.conn)?,
            "fingerprint_dup" => FingerprintOverlapSignal::count(&self.conn)?,
            "metadata_dup" => MetadataDuplicateSignal::count(&self.conn)?,
            "duplicate_inode" => DuplicateInodeSignal::count(&self.conn)?,
            "missing_tag" => MissingTagSignal::count(&self.conn)?,
            "deploy_conflict" => DeployConflictSignal::count(&self.conn)?,
            "tag_canonicity" => TagCanonicitySignal::count(&self.conn)?,
            "inconsistent_album_artist" => InconsistentAlbumArtistSignal::count(&self.conn)?,
            "compound_tag_value" => CompoundTagValueSignal::count(&self.conn)?,
            "cross_source_overlap" => CrossSourceOverlapSignal::count(&self.conn)?,
            "canonical_tag" => CanonicalTagSignal::count(&self.conn)?,
            "library_leftover" => LibraryLeftoverSignal::count(&self.conn)?,
            "library_stale" => LibraryStaleSignal::count(&self.conn)?,
            _ => 0,
        };
        Ok(count)
    }

    /// Count tracks affected by aggregate signals.
    ///
    /// Reads from typed tables and counts inodes in bincode BLOB data.
    fn count_affected_by_signal(&self, signal_type: &str) -> Result<usize> {
        let table = match signal_type {
            "metadata_dup" => "signal_metadata_duplicate",
            "duplicate_inode" => "signal_duplicate_inode",
            "missing_tag" => "signal_missing_tag",
            "deploy_conflict" => "signal_deploy_conflict",
            _ => return Ok(0),
        };
        // These tables store inodes in a bincode BLOB 'data' column.
        // Count rows and read blob to sum inode counts.
        let sql = format!("SELECT data FROM {}", table);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        })?;

        let mut total = 0usize;
        for row in rows {
            if let Ok(blob) = row {
                // All these types have an inodes: Vec<i64> field in their data
                if let Ok(inodes) = bincode::deserialize::<Vec<i64>>(&blob) {
                    total += inodes.len();
                }
            }
        }
        Ok(total)
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

        // Map signal type to its typed table name
        let table = match signal_type {
            "file_in_corpus" => "signal_file_in_corpus",
            "unindexed_file" => "signal_unindexed_file",
            "healthy_file" => "signal_healthy_file",
            "missing_file" => "signal_missing_file",
            _ => return Ok(DirectoryBreakdown { _entries: Vec::new() }),
        };

        // Extract parent directory from path column
        let sql = format!(
            r#"SELECT
                 CASE
                   WHEN instr(path, '/') > 0
                   THEN substr(path, 1, length(path) - length(replace(path, '/', '')) -
                        length(substr(path, length(path) - length(replace(path, '/', '')) + 1)))
                   ELSE ''
                 END as dir,
                 COUNT(*) as cnt
               FROM {}
               GROUP BY dir
               ORDER BY cnt DESC
               LIMIT 50"#,
            table
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let entries = stmt.query_map(params![], |row| {
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


    // ========================================================================
    // Deploy Modal Queries
    // ========================================================================

    /// Get all deploy-ready files (healthy corpus files not yet in library).
    ///
    /// Returns files with their corpus path and computed deploy path.
    /// Sorted by corpus_path for consistent display.
    pub fn get_deploy_ready_files(&self) -> Result<Vec<crate::corpus::db::types::DeploySignalFile>> {
        use crate::corpus::db::types::DeploySignalFile;

        let mut stmt = self.conn.prepare(
            "SELECT path, deploy_path FROM signal_deploy_ready ORDER BY path"
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(DeploySignalFile {
                corpus_path: row.get(0)?,
                deploy_path: row.get(1)?,
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

        let mut stmt = self.conn.prepare(
            "SELECT path, library_path FROM signal_deployed_healthy ORDER BY path"
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(DeploySignalFile {
                corpus_path: row.get(0)?,
                deploy_path: row.get(1)?,
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

        let mut stmt = self.conn.prepare(
            "SELECT library_path, expected_path FROM signal_library_stale ORDER BY library_path"
        )?;

        let results = stmt.query_map(params![], |row| {
            Ok(StaleSignalFile {
                library_path: row.get(0)?,
                expected_path: row.get(1)?,
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

        // key = "library_leftover:{library_name}:{library_path}"
        let mut stmt = self.conn.prepare(
            "SELECT key FROM signal_library_leftover ORDER BY key"
        )?;

        let results = stmt.query_map(params![], |row| {
            let key: String = row.get(0)?;
            // Extract library_path from key — paths start with '/'
            let library_path = if let Some(path_start) = key.find(":/") {
                key[path_start + 1..].to_string()
            } else {
                key
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

        let mut stmt = self.conn.prepare(
            "SELECT deploy_path, data FROM signal_deploy_conflict ORDER BY deploy_path"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let deploy_path: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((deploy_path, blob))
        })?;

        for row in rows {
            let (deploy_path, blob) = row?;
            let inodes: Vec<i64> = bincode::deserialize(&blob).unwrap_or_default();

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
            "SELECT path FROM signal_missing_file ORDER BY path"
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
            "SELECT path FROM signal_corrupt_file ORDER BY path"
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
            "SELECT path, file_type FROM signal_shit_format ORDER BY path"
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
            "SELECT file_type, COUNT(*) as cnt FROM signal_shit_format GROUP BY file_type ORDER BY cnt DESC"
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
        use crate::meta::signals::data::SubparDuplicateData;

        let mut stmt = self.conn.prepare(
            "SELECT path, data FROM signal_subpar_duplicate ORDER BY path"
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let path: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                let data: SubparDuplicateData = bincode::deserialize(&blob)
                    .unwrap_or_else(|_| SubparDuplicateData {
                        reason: "unknown".to_string(),
                        superior_inode: 0,
                        superior_path: String::new(),
                        dupe_group_fingerprint: String::new(),
                        quality_score: 0,
                        superior_quality_score: 0,
                    });
                Ok(SubparDuplicateEntry {
                    corpus_path: path,
                    reason: data.reason,
                    superior_path: data.superior_path,
                    _quality_score: data.quality_score as i64,
                    _superior_quality_score: data.superior_quality_score as i64,
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
        use crate::meta::signals::data::CanonicalTagSignal;
        use crate::meta::signals::store::AggregateSignalStore;
        let key = format!("{}:{}", tag_name, tag_value);
        Ok(CanonicalTagSignal::exists(&self.conn, &key).unwrap_or(false))
    }
}

// ============================================================================
// Signal Typed-Table Helpers
// ============================================================================

/// Get the typed table name for an aggregate signal type.
fn aggregate_signal_table_name(signal_type: AggregateSignalType) -> &'static str {
    match signal_type {
        AggregateSignalType::FingerprintOverlap => "signal_fingerprint_overlap",
        AggregateSignalType::MetadataDuplicate => "signal_metadata_duplicate",
        AggregateSignalType::DuplicateInode => "signal_duplicate_inode",
        AggregateSignalType::MissingTag => "signal_missing_tag",
        AggregateSignalType::DeployConflict => "signal_deploy_conflict",
        AggregateSignalType::TagCanonicity => "signal_tag_canonicity",
        AggregateSignalType::InconsistentAlbumArtist => "signal_inconsistent_album_artist",
        AggregateSignalType::CompoundTagValue => "signal_compound_tag_value",
        AggregateSignalType::CrossSourceOverlap => "signal_cross_source_overlap",
        AggregateSignalType::CanonicalTag => "signal_canonical_tag",
        AggregateSignalType::LibraryLeftover => "signal_library_leftover",
        AggregateSignalType::LibraryStale => "signal_library_stale",
    }
}

/// Whether an aggregate signal type stores a bincode data BLOB.
fn aggregate_signal_has_data_blob(signal_type: AggregateSignalType) -> bool {
    !matches!(signal_type,
        AggregateSignalType::CanonicalTag
        | AggregateSignalType::LibraryLeftover
        | AggregateSignalType::LibraryStale
    )
}

/// Reconstruct JSON metadata from bincode BLOB for backwards compatibility.
///
/// This is a transitional bridge — callers that consume metadata_json will be
/// migrated to use typed structs directly, at which point this goes away.
fn reconstruct_aggregate_metadata_json(
    signal_type: AggregateSignalType,
    key: &str,
    blob: &[u8],
) -> String {
    use crate::meta::signals::data::*;

    match signal_type {
        AggregateSignalType::FingerprintOverlap => {
            let inodes: Vec<i64> = bincode::deserialize(blob).unwrap_or_default();
            serde_json::json!({"inodes": inodes, "inode_count": inodes.len()}).to_string()
        }
        AggregateSignalType::MetadataDuplicate => {
            let data: MetadataDuplicateData = bincode::deserialize(blob).unwrap_or_else(|_| MetadataDuplicateData {
                tag_signature: String::new(), inodes: Vec::new(),
            });
            serde_json::json!({"tag_signature": data.tag_signature, "inodes": data.inodes, "inode_count": data.inodes.len()}).to_string()
        }
        AggregateSignalType::DuplicateInode => {
            let inodes: Vec<i64> = bincode::deserialize(blob).unwrap_or_default();
            // Extract inode from key
            let inode: i64 = key.parse().unwrap_or(0);
            serde_json::json!({"inode": inode, "inodes": inodes, "inode_count": inodes.len()}).to_string()
        }
        AggregateSignalType::MissingTag => {
            let data: MissingTagData = bincode::deserialize(blob).unwrap_or_else(|_| MissingTagData {
                missing_tags: Vec::new(), inodes: Vec::new(),
            });
            serde_json::json!({"missing_tags": data.missing_tags, "inodes": data.inodes, "inode_count": data.inodes.len()}).to_string()
        }
        AggregateSignalType::DeployConflict => {
            let inodes: Vec<i64> = bincode::deserialize(blob).unwrap_or_default();
            serde_json::json!({"deploy_path": key, "inodes": inodes, "inode_count": inodes.len()}).to_string()
        }
        AggregateSignalType::TagCanonicity => {
            let data: TagCanonicityData = bincode::deserialize(blob).unwrap_or_else(|_| TagCanonicityData {
                variants: Vec::new(), inodes: Vec::new(),
            });
            let variants_obj: serde_json::Map<String, serde_json::Value> = data.variants.into_iter()
                .map(|(k, v)| (k, serde_json::json!(v)))
                .collect();
            serde_json::json!({"variants": variants_obj, "inodes": data.inodes, "inode_count": data.inodes.len()}).to_string()
        }
        AggregateSignalType::InconsistentAlbumArtist => {
            let data: InconsistentAlbumArtistData = bincode::deserialize(blob).unwrap_or_else(|_| InconsistentAlbumArtistData {
                album: String::new(), artist_variants: Vec::new(), album_artist_variants: Vec::new(), inodes: Vec::new(),
            });
            let av: serde_json::Map<String, serde_json::Value> = data.artist_variants.into_iter().map(|(k, v)| (k, serde_json::json!(v))).collect();
            let aav: serde_json::Map<String, serde_json::Value> = data.album_artist_variants.into_iter().map(|(k, v)| (k, serde_json::json!(v))).collect();
            serde_json::json!({"album": data.album, "artist_variants": av, "album_artist_variants": aav, "inodes": data.inodes, "inode_count": data.inodes.len()}).to_string()
        }
        AggregateSignalType::CompoundTagValue => {
            let data: CompoundTagValueData = bincode::deserialize(blob).unwrap_or_else(|_| CompoundTagValueData {
                compound_value: String::new(), split_parts: Vec::new(), separator: String::new(), inodes: Vec::new(),
            });
            serde_json::json!({"compound_value": data.compound_value, "split_parts": data.split_parts, "separator": data.separator, "inodes": data.inodes, "inode_count": data.inodes.len()}).to_string()
        }
        AggregateSignalType::CrossSourceOverlap => {
            let data: CrossSourceOverlapData = bincode::deserialize(blob).unwrap_or_else(|_| CrossSourceOverlapData {
                source_a: String::new(), source_b: String::new(),
                source_a_can_stash: false, source_b_can_stash: false,
                overlap_count: 0, fingerprint_count: 0,
                fingerprint_keys: Vec::new(), track_pairs: Vec::new(),
            });
            serde_json::json!({
                "source_a": data.source_a, "source_b": data.source_b,
                "source_a_can_stash": data.source_a_can_stash, "source_b_can_stash": data.source_b_can_stash,
                "overlap_count": data.overlap_count, "fingerprint_count": data.fingerprint_count,
                "fingerprint_keys": data.fingerprint_keys,
                "track_pairs": data.track_pairs.iter().map(|tp| serde_json::json!({
                    "fingerprint_key": tp.fingerprint_key,
                    "source_a_inode": tp.source_a_inode, "source_a_path": tp.source_a_path,
                    "source_b_inode": tp.source_b_inode, "source_b_path": tp.source_b_path,
                })).collect::<Vec<_>>()
            }).to_string()
        }
        // These don't have BLOB data — shouldn't be called but handle gracefully
        _ => "{}".to_string(),
    }
}
