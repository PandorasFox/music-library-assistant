//! Health signal read-only queries.
//!
//! Signals are facts about corpus state stored in per-signal typed tables.
//! This module provides read-only query methods for the UI and computations.
//! Signal writes go through `db_thread` via `SignalWriteSender`.

use anyhow::Result;
use rusqlite::params;

use super::Database;
use crate::corpus::db::types::FileSource;

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
        total += FileInCorpusSignal::count(&self.conn).unwrap_or(0);
        total += UnindexedFileSignal::count(&self.conn).unwrap_or(0);
        total += HealthyFileSignal::count(&self.conn).unwrap_or(0);
        total += CorruptFileSignal::count(&self.conn).unwrap_or(0);
        total += MtimeOnlyMismatchSignal::count(&self.conn).unwrap_or(0);
        total += MissingDirectorySignal::count(&self.conn).unwrap_or(0);
        total += MissingFileSignal::count(&self.conn).unwrap_or(0);
        total += MovedFileSignal::count(&self.conn).unwrap_or(0);
        total += ShitFormatSignal::count(&self.conn).unwrap_or(0);
        total += DeployReadySignal::count(&self.conn).unwrap_or(0);
        total += DeployedHealthySignal::count(&self.conn).unwrap_or(0);
        total += OutOfBandTagSyncSignal::count(&self.conn).unwrap_or(0);
        total += OutOfBandTagConflictSignal::count(&self.conn).unwrap_or(0);
        total += SubparDuplicateSignal::count(&self.conn).unwrap_or(0);
        total += CompoundTagSignal::count(&self.conn).unwrap_or(0);
        // Aggregate signal tables
        total += FingerprintOverlapSignal::count(&self.conn).unwrap_or(0);
        total += MetadataDuplicateSignal::count(&self.conn).unwrap_or(0);
        total += DuplicateInodeSignal::count(&self.conn).unwrap_or(0);
        total += MissingTagSignal::count(&self.conn).unwrap_or(0);
        total += DeployConflictSignal::count(&self.conn).unwrap_or(0);
        total += TagCanonicitySignal::count(&self.conn).unwrap_or(0);
        total += InconsistentAlbumArtistSignal::count(&self.conn).unwrap_or(0);
        total += CrossSourceOverlapSignal::count(&self.conn).unwrap_or(0);
        total += RedundantDuplicateSignal::count(&self.conn).unwrap_or(0);
        total += CanonicalTagSignal::count(&self.conn).unwrap_or(0);
        total += LibraryLeftoverSignal::count(&self.conn).unwrap_or(0);
        total += LibraryStaleSignal::count(&self.conn).unwrap_or(0);
        total += EmbeddableAlbumArtSignal::count(&self.conn).unwrap_or(0);
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

    pub fn get_fingerprint_overlap_signals(&self) -> Result<Vec<crate::meta::signals::data::FingerprintOverlapSignal>> {
        crate::meta::signals::data::FingerprintOverlapSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query fingerprint overlap signals: {}", e))
    }

    /// Get compound tag signal groups aggregated by (tag_name, compound_value).
    ///
    /// Instead of returning one key per inode, groups all inodes sharing the same
    /// compound value so the operator decides once per unique value.
    ///
    /// If `safe_only` is true, returns only groups where the compound entry is safe.
    /// If false, returns only groups that need review.
    /// If `tag_filter` is Some, only returns groups for that specific tag name.
    ///
    /// Groups where the compound value has been marked canonical are excluded.
    pub fn get_compound_signal_groups_by_safety(
        &self,
        safe_only: bool,
        tag_filter: Option<&str>,
    ) -> Result<Vec<crate::meta::signals::data::CompoundGroup>> {
        use std::collections::HashMap;
        use crate::meta::signals::data::{CompoundTagEntry as TypedEntry, CompoundGroup};

        let mut stmt = self.conn.prepare(
            "SELECT inode, data FROM signal_compound_tag ORDER BY discovered_at DESC"
        )?;

        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((inode, blob))
        })?;

        // Group by (tag_name, compound_value) -> Vec<inode>
        // Use a Vec to preserve insertion order (discovered_at DESC)
        let mut group_order: Vec<(String, String)> = Vec::new();
        let mut group_map: HashMap<(String, String), Vec<i64>> = HashMap::new();

        for row in rows {
            let (inode, blob) = row?;

            let compounds: Vec<TypedEntry> = match bincode::deserialize(&blob) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if compounds.is_empty() {
                continue;
            }

            for compound in &compounds {
                // Skip canonical values
                if self.is_canonical_tag(&compound.tag_name, &compound.compound_value).unwrap_or(false) {
                    continue;
                }

                // Filter by safety
                if compound.is_safe() != safe_only {
                    continue;
                }

                // Filter by tag name
                if let Some(filter) = tag_filter {
                    if compound.tag_name != filter {
                        continue;
                    }
                }

                let key = (compound.tag_name.clone(), compound.compound_value.clone());
                let entry = group_map.entry(key.clone());
                use std::collections::hash_map::Entry;
                match entry {
                    Entry::Vacant(v) => {
                        v.insert(vec![inode]);
                        group_order.push(key);
                    }
                    Entry::Occupied(mut o) => {
                        let inodes = o.get_mut();
                        if !inodes.contains(&inode) {
                            inodes.push(inode);
                        }
                    }
                }
            }
        }

        let results = group_order
            .into_iter()
            .filter_map(|key| {
                let inodes = group_map.remove(&key)?;
                Some(CompoundGroup {
                    tag_name: key.0,
                    compound_value: key.1,
                    inodes,
                })
            })
            .collect();

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
        let oob_tag_conflict = self.count_signal_type("oob_tag_conflict")?;
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
        let subpar_duplicate_count = self.count_signal_type("subpar_duplicate")?;

        // Redundant duplicates (equal quality, requires operator choice)
        let redundant_duplicate_count = self.count_signal_type("redundant_duplicate")?;

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
            for (tag_name, blob) in rows.flatten() {
                let inode_count = bincode::deserialize::<crate::meta::signals::data::TagCanonicityData>(&blob)
                    .map(|d| d.inodes.len())
                    .unwrap_or(0);
                let entry = tag_map.entry(tag_name).or_insert((0, 0));
                entry.0 += 1; // cluster_count
                entry.1 += inode_count; // total_tracks
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

        let embeddable_album_art = self.count_signal_type("embeddable_album_art")?;

        Ok(TagSquashBucket {
            directory_overlap_cluster_count,
            subpar_duplicate_count,
            redundant_duplicate_count,
            tag_canonicity,
            inconsistent_album_artist_count,
            compound_tags,
            embeddable_album_art,
        })
    }

    /// Count unique compound tag values by safety classification, grouped by tag name.
    ///
    /// Counts unique (tag_name, compound_value) pairs rather than individual signals,
    /// so the insights view shows how many distinct compound values need resolution.
    /// Returns entries grouped by tag name, sorted by total count descending.
    fn count_compound_signals_by_tag(&self) -> Result<Vec<crate::corpus::db::types::CompoundTagEntry>> {
        use std::collections::{HashMap, HashSet};
        use crate::corpus::db::types::CompoundTagEntry;
        use crate::meta::signals::data::CompoundTagEntry as TypedEntry;

        let mut stmt = self.conn.prepare(
            "SELECT data FROM signal_compound_tag"
        )?;

        // Track unique (tag_name, compound_value) per safety bucket
        let mut safe_seen: HashSet<(String, String)> = HashSet::new();
        let mut review_seen: HashSet<(String, String)> = HashSet::new();

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
                let key = (compound.tag_name.clone(), compound.compound_value.clone());
                if compound.is_safe() {
                    safe_seen.insert(key);
                } else {
                    review_seen.insert(key);
                }
            }
        }

        // Aggregate counts per tag_name
        let mut by_tag: HashMap<String, (usize, usize)> = HashMap::new();
        for (tag_name, _) in &safe_seen {
            by_tag.entry(tag_name.clone()).or_insert((0, 0)).0 += 1;
        }
        for (tag_name, _) in &review_seen {
            by_tag.entry(tag_name.clone()).or_insert((0, 0)).1 += 1;
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
            "oob_tag_conflict" => OutOfBandTagConflictSignal::count(&self.conn)?,
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
            "cross_source_overlap" => CrossSourceOverlapSignal::count(&self.conn)?,
            "redundant_duplicate" => RedundantDuplicateSignal::count(&self.conn)?,
            "canonical_tag" => CanonicalTagSignal::count(&self.conn)?,
            "library_leftover" => LibraryLeftoverSignal::count(&self.conn)?,
            "library_stale" => LibraryStaleSignal::count(&self.conn)?,
            "embeddable_album_art" => EmbeddableAlbumArtSignal::count(&self.conn)?,
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
        for blob in rows.flatten() {
            // All these types have an inodes: Vec<i64> field in their data
            if let Ok(inodes) = bincode::deserialize::<Vec<i64>>(&blob) {
                total += inodes.len();
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
                library_name: String::new(), // populated by caller via config lookup
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
            let library_path: String = row.get(1)?;
            // library_path is "{library_name}/relative/path" — extract library_name
            let library_name = library_path.split('/').next().unwrap_or("").to_string();
            Ok(DeploySignalFile {
                library_name,
                corpus_path: row.get(0)?,
                deploy_path: library_path,
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
            let library_path: String = row.get(0)?;
            // library_path is "{library_name}/relative/path" — extract library_name
            let library_name = library_path.split('/').next().unwrap_or("").to_string();
            Ok(StaleSignalFile {
                library_name,
                library_path,
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

        // key = "library_leftover:{library_name}:{library_name}/path/..."
        let mut stmt = self.conn.prepare(
            "SELECT key FROM signal_library_leftover ORDER BY key"
        )?;

        let results = stmt.query_map(params![], |row| {
            let key: String = row.get(0)?;
            let (library_name, library_path) = crate::meta::signals::data::LibraryLeftoverSignal::parse_key(&key)
                .map(|(n, p)| (n.to_string(), p.to_string()))
                .unwrap_or_else(|| (String::new(), key.clone()));
            Ok(LeftoverSignalFile { library_name, library_path })
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
    /// Returns the path column for each missing_file signal.
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
    /// Returns the path column for each corrupt_file signal.
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
    /// Both values are typed columns in the signal_shit_format table.
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

    // ========================================================================
    // Embeddable Album Art Resolution Queries
    // ========================================================================

    /// Get all embeddable album art signals with deserialized data.
    pub fn get_embeddable_album_art_signals(&self) -> Result<Vec<crate::meta::signals::data::EmbeddableAlbumArtSignal>> {
        crate::meta::signals::data::EmbeddableAlbumArtSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query embeddable album art signals: {}", e))
    }

    /// Get corpus audio files that have no embedded pictures (has_pictures = 0).
    ///
    /// Returns (inode, absolute_path) for healthy corpus files without album art.
    /// Used by DetectEmbeddableAlbumArt to avoid lofty probing at Awake phase.
    pub fn get_artless_corpus_files(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT a.inode, f.path
               FROM audio_info a
               JOIN files f ON a.inode = f.inode
               JOIN signal_healthy_file h ON a.inode = h.inode
               WHERE f.source = 'corpus' AND a.has_pictures = 0"#
        )?;
        let rows = stmt.query_map(params![], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|e| anyhow::anyhow!("Failed to query artless corpus files: {}", e))
    }

    /// Get all inodes that have a CompoundTag signal containing a specific compound value.
    ///
    /// Used by EmitCanonicalTag mutation to find and clear stale CompoundTag signals
    /// after a value has been marked as canonical.
    pub fn get_inodes_with_compound_value(&self, tag_name: &str, compound_value: &str) -> Result<Vec<i64>> {
        use crate::meta::signals::data::CompoundTagEntry as TypedEntry;

        let mut stmt = self.conn.prepare(
            "SELECT inode, data FROM signal_compound_tag"
        )?;

        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((inode, blob))
        })?;

        let mut inodes = Vec::new();
        for row in rows {
            let (inode, blob) = row?;
            let compounds: Vec<TypedEntry> = match bincode::deserialize(&blob) {
                Ok(c) => c,
                Err(_) => continue,
            };
            if compounds.iter().any(|c| c.tag_name == tag_name && c.compound_value == compound_value) {
                inodes.push(inode);
            }
        }

        Ok(inodes)
    }

    // ========================================================================
    // Redundant Duplicate Resolution Queries
    // ========================================================================

    /// Get all redundant duplicate groups with deserialized data.
    ///
    /// Returns (signal_key, data) pairs for each group. Each group contains
    /// files with identical fingerprints and identical quality scores that
    /// require operator choice to resolve.
    pub fn get_redundant_duplicate_groups(&self) -> Result<Vec<(String, crate::meta::signals::data::RedundantDuplicateData)>> {
        use crate::meta::signals::data::RedundantDuplicateData;

        let mut stmt = self.conn.prepare(
            "SELECT key, data FROM signal_redundant_duplicate ORDER BY key"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let key: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((key, blob))
        })?;

        for row in rows {
            let (key, blob) = row?;
            if let Ok(data) = bincode::deserialize::<RedundantDuplicateData>(&blob) {
                results.push((key, data));
            }
        }

        Ok(results)
    }

    // ========================================================================
    // Metadata Duplicate Resolution Queries
    // ========================================================================

    /// Get all metadata duplicate groups with deserialized data.
    ///
    /// Returns (signal_key, data) pairs for each group. Each group contains
    /// files with identical tag signatures (artist/album/title) that may need
    /// tag editing or stashing to resolve.
    pub fn get_metadata_duplicate_groups(&self) -> Result<Vec<(String, crate::meta::signals::data::MetadataDuplicateData)>> {
        use crate::meta::signals::data::MetadataDuplicateData;

        let mut stmt = self.conn.prepare(
            "SELECT key, data FROM signal_metadata_duplicate ORDER BY key"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let key: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((key, blob))
        })?;

        for row in rows {
            let (key, blob) = row?;
            if let Ok(data) = bincode::deserialize::<MetadataDuplicateData>(&blob) {
                results.push((key, data));
            }
        }

        Ok(results)
    }
}

