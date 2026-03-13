//! Resolution modal queries: missing files, corrupt files, format issues,
//! duplicates, inbox corpus matches, and whitelist lookups.

use anyhow::Result;
use rusqlite::params;

use super::super::Database;

impl Database {
    /// Get all corpus paths with MissingFile signals.
    ///
    /// Returns the path column for each missing_file signal.
    /// Used by the missing file resolution modal to categorize files.
    /// MissingFile signals are keyed by inode with path in metadata.
    pub fn get_missing_file_paths(&self) -> Result<Vec<String>> {
        self.query_signal_paths("signal_missing_file")
    }

    /// Get all corpus paths with CorruptFile signals.
    ///
    /// Returns the path column for each corrupt_file signal.
    /// Used by the corrupt file resolution modal.
    /// CorruptFile signals are keyed by inode with path in metadata.
    pub fn get_corrupt_file_paths(&self) -> Result<Vec<String>> {
        self.query_signal_paths("signal_corrupt_file")
    }

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

    /// Get all redundant duplicate groups with deserialized data.
    pub fn get_redundant_duplicate_groups(
        &self,
    ) -> Result<Vec<(String, crate::meta::signals::data::RedundantDuplicateData)>> {
        self.query_signal_key_blobs(
            "SELECT key, data FROM signal_redundant_duplicate ORDER BY key",
        )
    }

    /// Get all cross-release recording groups with deserialized data.
    pub fn get_cross_release_recording_groups(
        &self,
    ) -> Result<Vec<(String, crate::meta::signals::data::CrossReleaseRecordingData)>> {
        self.query_signal_key_blobs(
            "SELECT key, data FROM signal_cross_release_recording ORDER BY key",
        )
    }

    /// Get all metadata duplicate groups with deserialized data.
    pub fn get_metadata_duplicate_groups(
        &self,
    ) -> Result<Vec<(String, crate::meta::signals::data::MetadataDuplicateData)>> {
        self.query_signal_key_blobs(
            "SELECT key, data FROM signal_metadata_duplicate ORDER BY key",
        )
    }

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
        use crate::meta::signals::data::InboxCorpusMatchData;
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

            let classification: MatchClassification = match_data.classification.into();

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
}
