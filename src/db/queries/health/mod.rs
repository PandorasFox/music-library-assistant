//! Health signal read-only queries.
//!
//! Signals are facts about corpus state stored in per-signal typed tables.
//! This module provides read-only query methods for the UI and computations.
//! Signal writes go through `db_thread` via `SignalWriteSender`.

use anyhow::Result;
use rusqlite::params;

use super::Database;
use crate::meta::signals::store::CorpusSignalStore;

/// Generate a typed signal query method on Database.
macro_rules! signal_query {
    (all, $fn_name:ident, $signal:ty) => {
        pub fn $fn_name(&self) -> Result<Vec<$signal>> {
            <$signal>::query_all(&self.conn)
                .map_err(|e| anyhow::anyhow!("Failed to query {} signals: {}", stringify!($signal), e))
        }
    };
    (by_key, $fn_name:ident, $signal:ty) => {
        pub fn $fn_name(&self, key: &str) -> Result<Option<$signal>> {
            <$signal>::query_by_key(&self.conn, key)
                .map_err(|e| anyhow::anyhow!("Failed to query {} signal: {}", stringify!($signal), e))
        }
    };
    (by_inode, $fn_name:ident, $signal:ty) => {
        pub fn $fn_name(&self, inode: i64) -> Result<Option<$signal>> {
            <$signal>::query_by_inode(&self.conn, inode)
                .map_err(|e| anyhow::anyhow!("Failed to query {} signal: {}", stringify!($signal), e))
        }
    };
}

mod deploy;
mod insights;
mod resolution_queries;

impl Database {
    // ========================================================================
    // Typed Signal Queries (direct struct access, no JSON)
    // ========================================================================

    /// Get all unindexed signal (inode, path) pairs for a zone.
    pub fn get_unindexed_signals_for<Z: crate::zones::AudioZone>(
        &self,
    ) -> Result<Vec<(i64, String)>> {
        use crate::meta::signals::store::CorpusSignalStore;
        let sql = format!(
            "SELECT inode, path FROM {} ORDER BY path",
            Z::UnindexedSignal::TABLE_NAME
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![], |row| Ok((row.get(0)?, row.get(1)?)))?;
        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    signal_query!(all, get_healthy_file_signals, crate::meta::signals::data::HealthyFileSignal);
    signal_query!(all, get_cross_source_overlap_signals, crate::meta::signals::data::CrossSourceOverlapSignal);
    signal_query!(all, get_release_overlap_signals, crate::meta::signals::data::ReleaseOverlapSignal);
    signal_query!(all, get_fingerprint_overlap_signals, crate::meta::signals::data::FingerprintOverlapSignal);
    signal_query!(all, get_missing_tag_signals, crate::meta::signals::data::MissingTagSignal);
    signal_query!(all, get_missing_album_single_signals, crate::meta::signals::data::MissingAlbumSingleSignal);
    signal_query!(all, get_disc_extraction_signals, crate::meta::signals::data::DiscExtractionSignal);

    signal_query!(by_key, get_tag_canonicity_signal, crate::meta::signals::data::TagCanonicitySignal);
    signal_query!(by_key, get_inconsistent_album_artist_signal, crate::meta::signals::data::InconsistentAlbumArtistSignal);
    signal_query!(by_key, get_inbox_tag_canonicity_signal, crate::meta::signals::data::InboxTagCanonicitySignal);

    signal_query!(by_inode, get_compound_tag_signal, crate::meta::signals::data::CompoundTagSignal);
    signal_query!(by_inode, get_inbox_compound_tag_signal, crate::meta::signals::data::InboxCompoundTagSignal);

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
        // Table name is a hardcoded literal, not user input — safe for direct interpolation.
        self.collect_compound_groups("signal_compound_tag", |compound| {
            if compound.is_safe() != safe_only {
                return false;
            }
            if let Some(filter) = tag_filter {
                if compound.tag_name != filter {
                    return false;
                }
            }
            true
        })
    }

    /// Get inbox compound tag signal groups aggregated by (tag_name, compound_value).
    ///
    /// Simplified version for inbox zone: no safe_only or tag_filter (inbox is small).
    /// Groups where the compound value has been marked canonical are excluded.
    pub fn get_inbox_compound_signal_groups(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::CompoundGroup>> {
        // Table name is a hardcoded literal, not user input — safe for direct interpolation.
        self.collect_compound_groups("signal_inbox_compound_tag", |_| true)
    }

    /// Shared accumulator for compound tag signal grouping.
    ///
    /// Deserializes compound tag entries from the given signal table, filters with
    /// `entry_filter`, skips canonical values, and groups by (tag_name, compound_value).
    /// Table name MUST be a hardcoded literal — never user input.
    fn collect_compound_groups(
        &self,
        table: &str,
        entry_filter: impl Fn(&crate::meta::signals::data::CompoundTagEntry) -> bool,
    ) -> Result<Vec<crate::meta::signals::data::CompoundGroup>> {
        use crate::meta::signals::data::{CompoundGroup, CompoundTagEntry as TypedEntry};
        use std::collections::hash_map::Entry;
        use std::collections::HashMap;

        let mut stmt = self.conn.prepare(&format!(
            "SELECT inode, data FROM {} ORDER BY discovered_at DESC",
            table
        ))?;

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

                if !entry_filter(compound) {
                    continue;
                }

                let key = (compound.tag_name.clone(), compound.compound_value.clone());
                let entry = group_map.entry(key.clone());
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
        self.query_signal_paths("signal_missing_directory")
    }

    /// Get all file-presence signal inodes with their paths, zone-generic.
    pub fn get_file_presence_inodes<Z: crate::zones::AudioZone>(
        &self,
    ) -> Result<std::collections::HashMap<i64, String>> {
        self.get_signal_inode_paths(Z::FilePresenceSignal::TABLE_NAME)
    }

    /// Get all signal inodes with their paths from a given signal table.
    fn get_signal_inode_paths(&self, table: &str) -> Result<std::collections::HashMap<i64, String>> {
        let sql = format!("SELECT inode, path FROM {}", table);
        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut result = std::collections::HashMap::new();
        for row in rows {
            let (inode, path) = row?;
            result.insert(inode, path);
        }
        Ok(result)
    }

    /// Query all `path` values from a signal table, sorted.
    ///
    /// Table name MUST be a hardcoded literal — never user input.
    fn query_signal_paths(&self, table: &str) -> Result<Vec<String>> {
        let sql = format!("SELECT path FROM {} ORDER BY path", table);
        let mut stmt = self.conn.prepare(&sql)?;
        let results = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        Ok(results)
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
    // External Match / MusicBrainz Data
    // ========================================================================

    /// Load all external match entries, bucketed by confidence tier for the lateral view.
    ///
    /// Reads `signal_external_match`, skips ExactMatch, buckets everything else
    /// by AcoustID confidence tier.
    pub fn get_external_matches_data(&self, config: &crate::config::Config) -> Result<crate::meta::views::ExternalMatchesData> {
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

        // Packing per-category counts via GROUP BY on key prefix.
        let (
            mut packing_perfect_count,
            mut packing_full_match_count,
            mut packing_singles_count,
            mut packing_incomplete_count,
            mut packing_low_confidence_count,
        ) = (0usize, 0, 0, 0, 0);
        for (cat, count) in self.count_by_category(
            "SELECT SUBSTR(key, 1, INSTR(key, ':') - 1) AS cat, COUNT(*) \
             FROM signal_packed_release GROUP BY cat",
        )? {
            match cat.as_str() {
                "perfect" => packing_perfect_count = count,
                "full_match" => packing_full_match_count = count,
                "single" => packing_singles_count = count,
                "incomplete" => packing_incomplete_count = count,
                "low_confidence" => packing_low_confidence_count = count,
                _ => {}
            }
        }

        let packing_knots_count: usize = self.conn
            .query_row("SELECT COUNT(*) FROM signal_packing_knot", [], |row| row.get(0))
            .unwrap_or(0);

        // Unsolved corpus tracks — single GROUP BY instead of 3 queries.
        let (mut unsolved_conflict_count, mut unsolved_no_release_count, mut unsolved_no_match_count) =
            (0usize, 0, 0);
        for (cat, count) in self.count_by_category(
            "SELECT category, COUNT(*) FROM signal_unmatched_corpus_track GROUP BY category",
        )? {
            match cat.as_str() {
                "conflict" => unsolved_conflict_count = count,
                "no_release" => unsolved_no_release_count = count,
                "no_match" => unsolved_no_match_count = count,
                _ => {}
            }
        }

        let va_override_count: usize = self.conn
            .query_row("SELECT COUNT(*) FROM signal_various_artists_override", [], |row| row.get(0))
            .unwrap_or(0);

        let pinned_conflict_count: usize = self.conn
            .query_row("SELECT COUNT(*) FROM signal_pinned_release_conflict", [], |row| row.get(0))
            .unwrap_or(0);

        // Check staleness: any pinned release without a matching packed_release signal?
        let pinned_ids: Vec<String> = config
            .source_dirs
            .iter()
            .filter_map(|sd| sd.pinned_release.clone())
            .collect();
        let pinned_releases_stale = if pinned_ids.is_empty() {
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

    /// Load external match signal entries filtered by confidence tier.
    ///
    /// Returns `(inode, path, confidence, recording_id)` tuples for all
    /// non-ExactMatch signals whose confidence passes the filter.
    /// MB cache enrichment is done by the caller.
    pub fn get_external_match_entries_filtered(
        &self,
        confidence: crate::meta::views::external_matches::AcoustidConfidence,
    ) -> Result<Vec<(i64, String, f64, String)>> {
        use crate::meta::signals::data::{ExternalMatchData, MatchClassification};

        let mut stmt = self.conn.prepare(
            "SELECT inode, path, data FROM signal_external_match ORDER BY path",
        )?;

        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((inode, path, blob))
        })?;

        let mut entries = Vec::new();
        for row in rows {
            let (inode, path, blob) = row?;
            let data: ExternalMatchData = match bincode::deserialize(&blob) {
                Ok(d) => d,
                Err(_) => continue,
            };

            if data.classification == MatchClassification::ExactMatch {
                continue;
            }

            if !confidence.matches(data.confidence) {
                continue;
            }

            entries.push((inode, path, data.confidence, data.recording_id));
        }

        Ok(entries)
    }

}
