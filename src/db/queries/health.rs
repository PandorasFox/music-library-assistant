//! Health signal read-only queries.
//!
//! Signals are facts about corpus state stored in per-signal typed tables.
//! This module provides read-only query methods for the UI and computations.
//! Signal writes go through `db_thread` via `SignalWriteSender`.

use anyhow::Result;
use rusqlite::params;

use super::Database;
use crate::db::types::Zone;

impl Database {
    // ========================================================================
    // Health Issue Operations
    // ========================================================================

    // ========================================================================
    // Typed Signal Queries (direct struct access, no JSON)
    // ========================================================================

    pub fn get_unindexed_file_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::UnindexedFileSignal>> {
        crate::meta::signals::data::UnindexedFileSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query unindexed file signals: {}", e))
    }

    pub fn get_healthy_file_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::HealthyFileSignal>> {
        crate::meta::signals::data::HealthyFileSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query healthy file signals: {}", e))
    }

    pub fn get_tag_canonicity_signal(
        &self,
        key: &str,
    ) -> Result<Option<crate::meta::signals::data::TagCanonicitySignal>> {
        crate::meta::signals::data::TagCanonicitySignal::query_by_key(&self.conn, key)
            .map_err(|e| anyhow::anyhow!("Failed to query tag canonicity signal: {}", e))
    }

    pub fn get_inconsistent_album_artist_signal(
        &self,
        key: &str,
    ) -> Result<Option<crate::meta::signals::data::InconsistentAlbumArtistSignal>> {
        crate::meta::signals::data::InconsistentAlbumArtistSignal::query_by_key(&self.conn, key)
            .map_err(|e| anyhow::anyhow!("Failed to query inconsistent album artist signal: {}", e))
    }

    pub fn get_compound_tag_signal(
        &self,
        inode: i64,
    ) -> Result<Option<crate::meta::signals::data::CompoundTagSignal>> {
        crate::meta::signals::data::CompoundTagSignal::query_by_inode(&self.conn, inode)
            .map_err(|e| anyhow::anyhow!("Failed to query compound tag signal: {}", e))
    }

    pub fn get_inbox_compound_tag_signal(
        &self,
        inode: i64,
    ) -> Result<Option<crate::meta::signals::data::InboxCompoundTagSignal>> {
        crate::meta::signals::data::InboxCompoundTagSignal::query_by_inode(&self.conn, inode)
            .map_err(|e| anyhow::anyhow!("Failed to query inbox compound tag signal: {}", e))
    }

    pub fn get_cross_source_overlap_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::CrossSourceOverlapSignal>> {
        crate::meta::signals::data::CrossSourceOverlapSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query cross source overlap signals: {}", e))
    }

    pub fn get_release_overlap_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::ReleaseOverlapSignal>> {
        crate::meta::signals::data::ReleaseOverlapSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query release overlap signals: {}", e))
    }

    pub fn get_fingerprint_overlap_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::FingerprintOverlapSignal>> {
        crate::meta::signals::data::FingerprintOverlapSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query fingerprint overlap signals: {}", e))
    }

    pub fn get_missing_tag_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::MissingTagSignal>> {
        crate::meta::signals::data::MissingTagSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query missing tag signals: {}", e))
    }

    pub fn get_missing_album_single_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::MissingAlbumSingleSignal>> {
        crate::meta::signals::data::MissingAlbumSingleSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query missing album single signals: {}", e))
    }

    pub fn get_disc_extraction_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::DiscExtractionSignal>> {
        crate::meta::signals::data::DiscExtractionSignal::query_all(&self.conn)
            .map_err(|e| anyhow::anyhow!("Failed to query disc extraction signals: {}", e))
    }

    pub fn get_inbox_tag_canonicity_signal(
        &self,
        key: &str,
    ) -> Result<Option<crate::meta::signals::data::InboxTagCanonicitySignal>> {
        crate::meta::signals::data::InboxTagCanonicitySignal::query_by_key(&self.conn, key)
            .map_err(|e| anyhow::anyhow!("Failed to query inbox tag canonicity signal: {}", e))
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
        use crate::meta::signals::data::{CompoundGroup, CompoundTagEntry as TypedEntry};
        use std::collections::HashMap;

        let mut stmt = self
            .conn
            .prepare("SELECT inode, data FROM signal_compound_tag ORDER BY discovered_at DESC")?;

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
                if self
                    .is_canonical_tag(&compound.tag_name, &compound.compound_value)
                    .unwrap_or(false)
                {
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

    /// Get inbox compound tag signal groups aggregated by (tag_name, compound_value).
    ///
    /// Simplified version for inbox zone: no safe_only or tag_filter (inbox is small).
    /// Groups where the compound value has been marked canonical are excluded.
    pub fn get_inbox_compound_signal_groups(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::CompoundGroup>> {
        use crate::meta::signals::data::{CompoundGroup, CompoundTagEntry as TypedEntry};
        use std::collections::HashMap;

        let mut stmt = self.conn.prepare(
            "SELECT inode, data FROM signal_inbox_compound_tag ORDER BY discovered_at DESC",
        )?;

        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((inode, blob))
        })?;

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
                if self
                    .is_canonical_tag(&compound.tag_name, &compound.compound_value)
                    .unwrap_or(false)
                {
                    continue;
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
               WHERE is_dir = 1 AND zone = 'corpus'"#,
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
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM signal_missing_directory ORDER BY path")?;
        let results = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(results)
    }

    /// Get all FileInCorpus signal inodes with their paths.
    ///
    /// Returns HashMap<inode, path> for set comparison operations.
    pub fn get_file_in_corpus_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode, path FROM signal_file_in_corpus")?;

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

    /// Get all FileInInbox signal inodes with their paths.
    pub fn get_file_in_inbox_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode, path FROM signal_file_in_inbox")?;

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

    /// Get all inbox unindexed files as (inode, path) pairs.
    pub fn get_inbox_unindexed_files(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode, path FROM signal_inbox_unindexed ORDER BY path")?;
        let rows = stmt.query_map(params![], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // Inbox Organizable Queries
    // ========================================================================

    /// Get inbox files eligible for organizing into the corpus.
    ///
    /// "Organizable" = has InboxHealthySignal AND:
    /// - has NO InboxCorpusMatchSignal, OR
    /// - has InboxCorpusMatchSignal classified as 'better' (inbox is higher quality)
    ///   AND is NOT referenced by InboxTagCanonicitySignal.
    ///
    /// Tag canonicity exclusion is done in Rust because inbox_inodes are
    /// stored in a bincode BLOB.
    pub fn get_organizable_inbox_files(&self) -> Result<Vec<(i64, String)>> {
        use crate::meta::signals::data::InboxTagCanonicityData;

        // Step 1: Get healthy inodes that either have no corpus match,
        // or have a corpus match classified as 'better' (inbox outranks corpus)
        let mut stmt = self.conn.prepare(
            "SELECT h.inode, h.path FROM signal_inbox_healthy h
             WHERE h.inode NOT IN (
                 SELECT inode FROM signal_inbox_corpus_match
                 WHERE classification != 'better'
             )",
        )?;
        let candidates: Vec<(i64, String)> = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Step 2: Load tag canonicity inodes from bincode blobs
        let mut tag_canon_inodes = std::collections::HashSet::new();
        let mut canon_stmt = self
            .conn
            .prepare("SELECT data FROM signal_inbox_tag_canonicity")?;
        let blobs: Vec<Vec<u8>> = canon_stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        for blob in blobs {
            if let Ok(data) = bincode::deserialize::<InboxTagCanonicityData>(&blob) {
                for inode in &data.inbox_inodes {
                    tag_canon_inodes.insert(*inode);
                }
            }
        }

        // Step 3: Filter out tag canonicity inodes
        let result = if tag_canon_inodes.is_empty() {
            candidates
        } else {
            candidates
                .into_iter()
                .filter(|(inode, _)| !tag_canon_inodes.contains(inode))
                .collect()
        };

        Ok(result)
    }

    /// Count inbox files eligible for organizing into the corpus.
    pub fn get_organizable_inbox_count(&self) -> Result<usize> {
        self.get_organizable_inbox_files().map(|v| v.len())
    }

    // ========================================================================
    // Inbox Overview Data
    // ========================================================================

    /// Get InboxOverviewData for the Inbox view.
    ///
    /// Computes signal counts via typed table counts. Called by UiReadCache.
    pub fn get_inbox_overview_data(&self) -> Result<crate::meta::views::InboxOverviewData> {
        Ok(crate::meta::views::InboxOverviewData {
            file_in_inbox: self.count_signal_type("file_in_inbox")?,
            corpus_match: self.count_signal_type("inbox_corpus_match")?,
            unindexed: self.count_signal_type("inbox_unindexed")?,
            tag_canonicity: self.count_signal_type("inbox_tag_canonicity")?,
            missing_tags: self.count_signal_type("inbox_missing_tag")?,
            compound_tags: self.count_signal_type("inbox_compound_tag")?,
            organizable: self.get_organizable_inbox_count().unwrap_or(0),
        })
    }

    // ========================================================================
    // Insights Data
    // ========================================================================

    /// Get InsightsData for the Insights view.
    ///
    /// Computes all bucket data via SQL queries. Called by UiReadCache.
    pub fn get_insights_data(&self) -> Result<crate::meta::views::InsightsData> {
        use crate::meta::views::*;

        Ok(InsightsData {
            bucket_corpus: self.compute_corpus_files_bucket()?,
            bucket_placeholder: self.compute_tag_resolution_bucket()?,
            bucket_other: self.compute_other_signals_bucket()?,
        })
    }

    fn compute_corpus_files_bucket(&self) -> Result<crate::meta::views::CorpusFilesBucket> {
        use crate::meta::views::*;

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

        // Image files
        let images_in_corpus = self.get_image_file_count(Some("corpus")).unwrap_or(0);

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
            images_in_corpus,
            file_type_breakdown,
            _directory_breakdown: directory_breakdown,
        })
    }

    fn compute_tag_resolution_bucket(&self) -> Result<crate::meta::views::TagSquashBucket> {
        use crate::meta::views::*;

        // Cross-source overlap clusters (easy resolutions - at top of bucket)
        // These are derived from fingerprint overlaps, clustered by source directory
        let directory_overlap_cluster_count = self.count_signal_type("cross_source_overlap")?;

        // Release overlaps (multiple releases → same album directory)
        let release_overlap_count = self.count_signal_type("release_overlap")?;

        // Subpar duplicates (lower quality versions identified by fingerprint analysis)
        let subpar_duplicate_count = self.count_signal_type("subpar_duplicate")?;

        // Redundant duplicates (equal quality, requires operator choice)
        let redundant_duplicate_count = self.count_signal_type("redundant_duplicate")?;

        // Count inconsistent_album_artist signals
        let inconsistent_album_artist_count =
            self.count_signal_type("inconsistent_album_artist")?;

        // Group compound_tag signals by tag name with safety classification
        let compound_tags = self.count_compound_signals_by_tag()?;

        // Group tag_canonicity signals by tag_name column
        // Sum inodes from bincode BLOB data
        let mut tag_map: std::collections::HashMap<String, (usize, usize)> =
            std::collections::HashMap::new();
        {
            let mut stmt = self
                .conn
                .prepare("SELECT tag_name, data FROM signal_tag_canonicity")?;
            let rows = stmt.query_map(params![], |row| {
                let tag_name: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                Ok((tag_name, blob))
            })?;
            for (tag_name, blob) in rows.flatten() {
                let inode_count =
                    bincode::deserialize::<crate::meta::signals::data::TagCanonicityData>(&blob)
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

        let missing_album_single_count = self.count_signal_type("missing_album_single")?;

        let disc_extraction_count = self.count_signal_type("disc_extraction")?;

        let path_tag_mismatch_count = self.count_signal_type("path_tag_mismatch")?;

        Ok(TagSquashBucket {
            directory_overlap_cluster_count,
            release_overlap_count,
            subpar_duplicate_count,
            redundant_duplicate_count,
            tag_canonicity,
            inconsistent_album_artist_count,
            compound_tags,
            missing_album_single_count,
            disc_extraction_count,
            path_tag_mismatch_count,
        })
    }

    /// Count unique compound tag values by safety classification, grouped by tag name.
    ///
    /// Counts unique (tag_name, compound_value) pairs rather than individual signals,
    /// so the insights view shows how many distinct compound values need resolution.
    /// Returns entries grouped by tag name, sorted by total count descending.
    fn count_compound_signals_by_tag(&self) -> Result<Vec<crate::meta::views::CompoundTagEntry>> {
        use crate::meta::signals::data::CompoundTagEntry as TypedEntry;
        use crate::meta::views::CompoundTagEntry;
        use std::collections::{HashMap, HashSet};

        let mut stmt = self.conn.prepare("SELECT data FROM signal_compound_tag")?;

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

    fn compute_other_signals_bucket(&self) -> Result<crate::meta::views::OtherSignalsBucket> {
        use crate::meta::views::*;

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

    /// Load all external match entries, bucketed by confidence tier for the lateral view.
    ///
    /// Reads `signal_external_match`, skips ExactMatch, buckets everything else
    /// by AcoustID confidence tier.
    pub fn get_external_matches_data(&self) -> Result<crate::meta::views::ExternalMatchesData> {
        use crate::meta::signals::data::{ExternalMatchData, MatchClassification};
        use crate::meta::views::{
            ConfidenceBucket, ConfidenceTier, ExternalMatchReviewEntry, ExternalMatchesData,
        };
        use std::collections::HashMap;

        let mut stmt = self
            .conn
            .prepare("SELECT inode, path, data FROM signal_external_match ORDER BY path")?;

        let mut tier_map: HashMap<ConfidenceTier, Vec<ExternalMatchReviewEntry>> = HashMap::new();
        let mut untagged_entries: Vec<ExternalMatchReviewEntry> = Vec::new();

        let rows = stmt.query_map(params![], |row| {
            let path: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((path, blob))
        })?;

        for row in rows {
            let (path, blob) = row?;
            let data: ExternalMatchData = match bincode::deserialize(&blob) {
                Ok(d) => d,
                Err(_) => continue,
            };

            if data.classification == MatchClassification::ExactMatch {
                continue;
            }

            let entry = ExternalMatchReviewEntry {
                path,
                confidence: data.confidence,
                recording_id: data.recording_id,
            };

            if data.classification == MatchClassification::MetadataOnly {
                untagged_entries.push(entry);
            } else {
                let tier = ConfidenceTier::from_confidence(entry.confidence);
                tier_map.entry(tier).or_default().push(entry);
            }
        }

        let confidence_buckets: Vec<ConfidenceBucket> = ConfidenceTier::ALL
            .iter()
            .filter_map(|&tier| {
                let entries = tier_map.remove(&tier)?;
                let total = entries.len();
                Some(ConfidenceBucket {
                    tier,
                    total,
                    entries,
                })
            })
            .collect();

        // Packing per-category counts from PackedRelease aggregate signals
        let packing_perfect_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_packed_release WHERE key LIKE 'perfect:%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let packing_full_match_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_packed_release WHERE key LIKE 'full_match:%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let packing_singles_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_packed_release WHERE key LIKE 'single:%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let packing_incomplete_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_packed_release WHERE key LIKE 'incomplete:%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let packing_low_confidence_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_packed_release WHERE key LIKE 'low_confidence:%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let packing_knots_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_packing_knot",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Unsolved corpus tracks — counted directly from the category column.
        let unsolved_conflict_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_unmatched_corpus_track WHERE category = 'conflict'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let unsolved_no_release_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_unmatched_corpus_track WHERE category = 'no_release'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let unsolved_no_match_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_unmatched_corpus_track WHERE category = 'no_match'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let va_override_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_various_artists_override",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let pinned_conflict_count: usize = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM signal_pinned_release_conflict",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        // Check staleness: any pinned release without a matching packed_release signal?
        let pinned_releases_stale = match crate::config::load_config() {
            Ok(cfg) => {
                let pinned_ids: Vec<String> = cfg
                    .source_dirs
                    .iter()
                    .filter_map(|sd| sd.pinned_release.clone())
                    .collect();
                if pinned_ids.is_empty() {
                    false
                } else {
                    pinned_ids.iter().any(|rid| {
                        let exists: bool = self
                            .conn
                            .query_row(
                                "SELECT EXISTS(SELECT 1 FROM signal_packed_release WHERE key LIKE '%:' || ?1)",
                                [rid],
                                |row| row.get(0),
                            )
                            .unwrap_or(false);
                        !exists
                    })
                }
            }
            Err(_) => false,
        };

        Ok(ExternalMatchesData {
            untagged_entries,
            confidence_buckets,
            packing_perfect_count,
            packing_full_match_count,
            packing_singles_count,
            packing_incomplete_count,
            packing_low_confidence_count,
            packing_knots_count,
            unsolved_conflict_count,
            unsolved_no_release_count,
            unsolved_no_match_count,
            va_override_count,
            pinned_conflict_count,
            pinned_releases_stale,
        })
    }

    /// Count signals of a specific type using typed tables.
    fn count_signal_type(&self, signal_type: &str) -> Result<usize> {
        use crate::meta::signals::data::*;
        use crate::meta::signals::store::{AggregateSignalStore, CorpusSignalStore};
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
            "sidecar_deploy_conflict" => SidecarDeployConflictSignal::count(&self.conn)?,
            "tag_canonicity" => TagCanonicitySignal::count(&self.conn)?,
            "inconsistent_album_artist" => InconsistentAlbumArtistSignal::count(&self.conn)?,
            "cross_source_overlap" => CrossSourceOverlapSignal::count(&self.conn)?,
            "release_overlap" => ReleaseOverlapSignal::count(&self.conn)?,
            "redundant_duplicate" => RedundantDuplicateSignal::count(&self.conn)?,
            "canonical_tag" => CanonicalTagSignal::count(&self.conn)?,
            "library_leftover" => LibraryLeftoverSignal::count(&self.conn)?,
            "library_stale" => LibraryStaleSignal::count(&self.conn)?,
            "missing_album_single" => MissingAlbumSingleSignal::count(&self.conn)?,
            "expected_missing_tag" => ExpectedMissingTagSignal::count(&self.conn)?,
            "inbox_unindexed" => InboxUnindexedSignal::count(&self.conn)?,
            "inbox_healthy" => InboxHealthySignal::count(&self.conn)?,
            "inbox_corpus_match" => InboxCorpusMatchSignal::count(&self.conn)?,
            "file_in_inbox" => FileInInboxSignal::count(&self.conn)?,
            "inbox_tag_canonicity" => InboxTagCanonicitySignal::count(&self.conn)?,
            "inbox_missing_tag" => InboxMissingTagSignal::count(&self.conn)?,
            "inbox_compound_tag" => InboxCompoundTagSignal::count(&self.conn)?,
            "disc_extraction" => DiscExtractionSignal::count(&self.conn)?,
            "path_tag_mismatch" => PathTagMismatchSignal::count(&self.conn)?,
            "external_match" => ExternalMatchSignal::count(&self.conn)?,
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
            "sidecar_deploy_conflict" => "signal_sidecar_deploy_conflict",
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
               WHERE f.zone = 'corpus' AND f.is_dir = 0
               GROUP BY a.file_type
               ORDER BY cnt DESC"#,
        )?;

        let results = stmt
            .query_map(params![], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get directory breakdown for a signal type.
    fn get_directory_breakdown(
        &self,
        signal_type: &str,
    ) -> Result<crate::meta::views::DirectoryBreakdown> {
        use crate::meta::views::*;

        // Map signal type to its typed table name
        let table = match signal_type {
            "file_in_corpus" => "signal_file_in_corpus",
            "unindexed_file" => "signal_unindexed_file",
            "healthy_file" => "signal_healthy_file",
            "missing_file" => "signal_missing_file",
            _ => {
                return Ok(DirectoryBreakdown {
                    _entries: Vec::new(),
                })
            }
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
        let entries = stmt
            .query_map(params![], |row| {
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
    pub fn get_deploy_ready_files(&self) -> Result<Vec<crate::meta::views::DeploySignalFile>> {
        use crate::meta::views::DeploySignalFile;

        let mut stmt = self
            .conn
            .prepare("SELECT path, deploy_path FROM signal_deploy_ready ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| {
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
    pub fn get_deployed_healthy_files(&self) -> Result<Vec<crate::meta::views::DeploySignalFile>> {
        use crate::meta::views::DeploySignalFile;

        let mut stmt = self
            .conn
            .prepare("SELECT path, library_path FROM signal_deployed_healthy ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| {
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

    /// Get all sidecar images ready for deployment.
    ///
    /// Reads precomputed signals emitted by DeriveCorpusDeployStatus.
    pub fn get_sidecar_deploy_ready_signals(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::SidecarDeployReadySignal>> {
        use crate::meta::signals::data::{SidecarDeployReadyData, SidecarDeployReadySignal};

        let mut stmt = self.conn.prepare(
            "SELECT inode, path, deploy_path, library_name, data FROM signal_sidecar_deploy_ready ORDER BY path"
        )?;

        let results = stmt
            .query_map(params![], |row| {
                let blob: Vec<u8> = row.get(4)?;
                let data: SidecarDeployReadyData = bincode::deserialize(&blob).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Blob,
                        Box::new(e),
                    )
                })?;
                Ok(SidecarDeployReadySignal {
                    inode: row.get(0)?,
                    path: row.get(1)?,
                    deploy_path: row.get(2)?,
                    library_name: row.get(3)?,
                    data,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all stale library files (deployed at wrong path due to tag changes).
    ///
    /// Returns files with their current library path and expected path.
    /// Sorted by library_path for consistent display.
    pub fn get_library_stale_files(&self) -> Result<Vec<crate::meta::views::StaleSignalFile>> {
        use crate::meta::views::StaleSignalFile;

        let mut stmt = self.conn.prepare(
            "SELECT library_path, expected_path FROM signal_library_stale ORDER BY library_path",
        )?;

        let results = stmt
            .query_map(params![], |row| {
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
    pub fn get_library_leftover_files(
        &self,
    ) -> Result<Vec<crate::meta::views::LeftoverSignalFile>> {
        use crate::meta::views::LeftoverSignalFile;

        // key = "library_leftover:{library_name}:{library_name}/path/..."
        let mut stmt = self
            .conn
            .prepare("SELECT key FROM signal_library_leftover ORDER BY key")?;

        let results = stmt
            .query_map(params![], |row| {
                let key: String = row.get(0)?;
                let (library_name, library_path) =
                    crate::meta::signals::data::LibraryLeftoverSignal::parse_key(&key)
                        .map(|(n, p)| (n.to_string(), p.to_string()))
                        .unwrap_or_else(|| (String::new(), key.clone()));
                Ok(LeftoverSignalFile {
                    library_name,
                    library_path,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(results)
    }

    /// Get all deploy conflict groups (multiple corpus files → same library path).
    ///
    /// Sorted by deploy_path for consistent display.
    pub fn get_deploy_conflict_groups(&self) -> Result<Vec<crate::meta::views::ConflictGroup>> {
        use crate::meta::views::ConflictGroup;

        let mut stmt = self
            .conn
            .prepare("SELECT deploy_path, data FROM signal_deploy_conflict ORDER BY deploy_path")?;

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
                if let Ok(Some(audio_file)) = self.get_audio_file_by_inode(inode, Zone::Corpus) {
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

    /// Get all sidecar deploy conflict groups (multiple corpus images → same library path).
    ///
    /// Sorted by library_name/deploy_path for consistent display.
    pub fn get_sidecar_conflict_groups(
        &self,
    ) -> Result<Vec<crate::meta::views::SidecarConflictGroup>> {
        use crate::meta::views::SidecarConflictGroup;

        let mut stmt = self.conn.prepare(
            "SELECT deploy_path, library_name, data FROM signal_sidecar_deploy_conflict ORDER BY library_name, deploy_path"
        )?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let deploy_path: String = row.get(0)?;
            let library_name: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((deploy_path, library_name, blob))
        })?;

        for row in rows {
            let (deploy_path, library_name, blob) = row?;
            let inodes: Vec<i64> = bincode::deserialize(&blob).unwrap_or_default();

            let mut conflicting_files = Vec::new();
            for inode in inodes {
                if let Ok(Some(path)) = self.get_corpus_path_for_inode(inode) {
                    conflicting_files.push((path, inode));
                }
            }

            results.push(SidecarConflictGroup {
                deploy_path,
                library_name,
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
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM signal_missing_file ORDER BY path")?;

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
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM signal_corrupt_file ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;

        Ok(results)
    }

    // ========================================================================
    // Shit Format Resolution Queries
    // ========================================================================

    /// Get all ShitFormat signals with their inodes.
    ///
    /// Returns (inode, signal_path, file_type) for each shit_format signal.
    /// The signal_path may be stale if files were reorganized after signal emission;
    /// callers should look up the current path via inode from the files table.
    pub fn get_shit_format_files(&self) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode, path, file_type FROM signal_shit_format ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<(i64, String, String)>>>()?;

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
    pub fn get_subpar_duplicate_files(
        &self,
    ) -> Result<Vec<crate::meta::views::SubparDuplicateEntry>> {
        use crate::meta::signals::data::SubparDuplicateData;
        use crate::meta::views::SubparDuplicateEntry;

        let mut stmt = self
            .conn
            .prepare("SELECT path, data FROM signal_subpar_duplicate ORDER BY path")?;

        let results = stmt
            .query_map(params![], |row| {
                let path: String = row.get(0)?;
                let blob: Vec<u8> = row.get(1)?;
                let data: SubparDuplicateData =
                    bincode::deserialize(&blob).unwrap_or_else(|_| SubparDuplicateData {
                        reason: "unknown".to_string(),
                        superior_inode: 0,
                        superior_path: String::new(),
                        dupe_group_fingerprint: String::new(),
                        similarity_score: 0.0,
                    });
                Ok(SubparDuplicateEntry {
                    corpus_path: path,
                    reason: data.reason,
                    superior_path: data.superior_path,
                    similarity_score: data.similarity_score,
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
    /// Uses normalized tag name in the key (strips separators + uppercases) so that
    /// lookups match regardless of compound tag name variant:
    /// "ALBUMARTIST:value" ≈ "album_artist:value" ≈ "ALBUM_ARTIST:value".
    ///
    /// Used to skip compound tag detection for operator-confirmed canonical values.
    /// For example, if "artist:Rinse & Repeat" is marked canonical, we shouldn't
    /// flag it for splitting even though it contains " & ".
    pub fn is_canonical_tag(&self, tag_name: &str, tag_value: &str) -> Result<bool> {
        use crate::meta::signals::data::CanonicalTagSignal;
        use crate::meta::signals::store::AggregateSignalStore;
        let normalized = mm_utils::tag_names::normalize_tag_name(tag_name);
        let key = format!("{}:{}", normalized, tag_value);
        Ok(CanonicalTagSignal::exists(&self.conn, &key).unwrap_or(false))
    }

    // ========================================================================
    // ExpectedOverlap Whitelist Queries
    // ========================================================================

    /// Check if an ExpectedOverlap signal exists for this source pair key.
    ///
    /// Used by DetectCrossSourceOverlaps to skip expected source pair overlaps.
    pub fn is_expected_overlap(&self, pair_key: &str) -> Result<bool> {
        use crate::meta::signals::data::ExpectedOverlapSignal;
        use crate::meta::signals::store::AggregateSignalStore;
        Ok(ExpectedOverlapSignal::exists(&self.conn, pair_key).unwrap_or(false))
    }

    // ========================================================================
    // ExpectedDuplicate Whitelist Queries
    // ========================================================================

    /// Check if an ExpectedDuplicate signal exists for this fingerprint key.
    ///
    /// Used by AnalyzeFingerprintOverlaps to skip expected fingerprint overlap groups.
    pub fn is_expected_duplicate(&self, fingerprint_key: &str) -> Result<bool> {
        use crate::meta::signals::data::ExpectedDuplicateSignal;
        use crate::meta::signals::store::AggregateSignalStore;
        Ok(ExpectedDuplicateSignal::exists(&self.conn, fingerprint_key).unwrap_or(false))
    }

    // ========================================================================
    /// Get all inodes that have a CompoundTag signal containing a specific compound value.
    ///
    /// Used by EmitCanonicalTag mutation to find and clear stale CompoundTag signals
    /// after a value has been marked as canonical.
    pub fn get_inodes_with_compound_value(
        &self,
        tag_name: &str,
        compound_value: &str,
    ) -> Result<Vec<i64>> {
        use crate::meta::signals::data::CompoundTagEntry as TypedEntry;

        let mut stmt = self
            .conn
            .prepare("SELECT inode, data FROM signal_compound_tag")?;

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
            let normalized = mm_utils::tag_names::normalize_tag_name(tag_name);
            if compounds.iter().any(|c| {
                mm_utils::tag_names::normalize_tag_name(&c.tag_name) == normalized
                    && c.compound_value == compound_value
            }) {
                inodes.push(inode);
            }
        }

        Ok(inodes)
    }

    // ========================================================================
    // Inbox Corpus Match Resolution Queries
    // ========================================================================

    /// Get all inbox corpus match entries with quality classification.
    ///
    /// Reads InboxCorpusMatch signals, deserializes bincode BLOB data,
    /// and reads the pre-computed classification from the signal. Quality
    /// strings are still looked up from audio_info for display purposes.
    ///
    /// `bitrate_fuzz_percent` is no longer used for classification (now
    /// pre-computed at signal emission time) but kept in the signature
    /// for API compatibility.
    pub fn get_inbox_corpus_match_entries(
        &self,
        _bitrate_fuzz_percent: f64,
    ) -> Result<Vec<crate::meta::views::InboxCorpusMatchEntry>> {
        use crate::meta::signals::data::{CorpusMatchQuality, InboxCorpusMatchData};
        use crate::meta::views::{CorpusMatchDetail, InboxCorpusMatchEntry, MatchClassification};

        let mut stmt = self
            .conn
            .prepare("SELECT inode, path, data FROM signal_inbox_corpus_match ORDER BY path")?;

        let mut results = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((inode, path, blob))
        })?;

        for row in rows {
            let (inbox_inode, inbox_path, blob) = row?;

            let match_data: InboxCorpusMatchData = match bincode::deserialize(&blob) {
                Ok(d) => d,
                Err(_) => continue,
            };

            if match_data.corpus_matches.is_empty() {
                continue;
            }

            // Read pre-computed classification from signal data
            let classification = match match_data.classification {
                CorpusMatchQuality::Better => MatchClassification::Better,
                CorpusMatchQuality::Equivalent => MatchClassification::Equivalent,
                CorpusMatchQuality::Subpar => MatchClassification::Subpar,
            };

            // Look up quality strings for display only
            let inbox_quality = self.get_quality_string(inbox_inode);

            let corpus_details: Vec<CorpusMatchDetail> = match_data
                .corpus_matches
                .iter()
                .map(|cm| CorpusMatchDetail {
                    _corpus_inode: cm.corpus_inode,
                    corpus_path: cm.corpus_path.clone(),
                    corpus_quality: self.get_quality_string(cm.corpus_inode),
                    similarity: cm.similarity,
                })
                .collect();

            results.push(InboxCorpusMatchEntry {
                inbox_inode,
                inbox_path,
                inbox_quality,
                corpus_matches: corpus_details,
                classification,
            });
        }

        Ok(results)
    }

    /// Get a human-readable quality string for an inode from audio_info.
    fn get_quality_string(&self, inode: i64) -> String {
        let mut stmt = match self
            .conn
            .prepare("SELECT file_type, bitrate_kbps, sample_rate FROM audio_info WHERE inode = ?1")
        {
            Ok(s) => s,
            Err(_) => return "Unknown".to_string(),
        };

        match stmt.query_row(params![inode], |row| {
            let file_type: String = row.get(0)?;
            let bitrate: Option<i32> = row.get(1)?;
            let sample_rate: Option<i32> = row.get(2)?;
            Ok((file_type, bitrate, sample_rate))
        }) {
            Ok((file_type, bitrate, sample_rate)) => {
                let ft = file_type.to_uppercase();
                match (bitrate, sample_rate) {
                    (Some(br), Some(sr)) => format!("{} {}kbps {}Hz", ft, br, sr),
                    (Some(br), None) => format!("{} {}kbps", ft, br),
                    (None, Some(sr)) => format!("{} {}Hz", ft, sr),
                    (None, None) => ft,
                }
            }
            Err(_) => "Unknown".to_string(),
        }
    }

    // ========================================================================
    // Redundant Duplicate Resolution Queries
    // ========================================================================

    /// Get all redundant duplicate groups with deserialized data.
    ///
    /// Returns (signal_key, data) pairs for each group. Each group contains
    /// files with identical fingerprints and identical quality scores that
    /// require operator choice to resolve.
    pub fn get_redundant_duplicate_groups(
        &self,
    ) -> Result<Vec<(String, crate::meta::signals::data::RedundantDuplicateData)>> {
        use crate::meta::signals::data::RedundantDuplicateData;

        let mut stmt = self
            .conn
            .prepare("SELECT key, data FROM signal_redundant_duplicate ORDER BY key")?;

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
    pub fn get_metadata_duplicate_groups(
        &self,
    ) -> Result<Vec<(String, crate::meta::signals::data::MetadataDuplicateData)>> {
        use crate::meta::signals::data::MetadataDuplicateData;

        let mut stmt = self
            .conn
            .prepare("SELECT key, data FROM signal_metadata_duplicate ORDER BY key")?;

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

    /// Get deploy status for the Deploy view and titlebar indicator.
    ///
    /// Returns whether there's actionable deploy work and per-library file counts.
    pub fn get_deploy_status(&self) -> Result<crate::meta::views::DeployStatus> {
        let needs_action: bool = self.conn.query_row(
            "SELECT
                EXISTS(SELECT 1 FROM signal_deploy_ready)
                OR EXISTS(SELECT 1 FROM signal_library_stale)
                OR EXISTS(SELECT 1 FROM signal_library_leftover)
                OR EXISTS(SELECT 1 FROM signal_sidecar_deploy_ready)",
            [],
            |row| row.get(0),
        )?;

        // Per-library file counts: extract library_name from the path prefix before first '/'
        let mut stmt = self.conn.prepare(
            "SELECT
                CASE
                    WHEN INSTR(path, '/') > 0 THEN SUBSTR(path, 1, INSTR(path, '/') - 1)
                    ELSE path
                END AS library_name,
                COUNT(*) AS cnt
            FROM files
            WHERE zone = 'library'
            GROUP BY library_name
            ORDER BY library_name",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, usize>(1)?))
        })?;

        let mut library_file_counts = Vec::new();
        for row in rows {
            library_file_counts.push(row?);
        }

        Ok(crate::meta::views::DeployStatus {
            needs_action,
            library_file_counts,
        })
    }
}
