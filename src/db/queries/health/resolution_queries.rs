//! Resolution modal queries: missing files, corrupt files, format issues,
//! duplicates, and whitelist lookups.

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

    /// Get all corrupt file signals with inode + path.
    ///
    /// CorruptFile signals are keyed by inode with path in metadata.
    /// Returns (inode, path) pairs directly — no filesystem access needed.
    pub fn get_corrupt_file_signals(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode, path FROM signal_corrupt_file ORDER BY path")?;
        let results = stmt
            .query_map(params![], |row| {
                Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(results)
    }

    /// Get all LosslessRemux signals with their inodes.
    ///
    /// Returns (inode, signal_path, file_type) for each lossless_remux signal.
    /// The signal_path may be stale if files were reorganized after signal emission;
    /// callers should look up the current path via inode from the files table.
    pub fn get_lossless_remux_files(&self) -> Result<Vec<(i64, String, String)>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode, path, file_type FROM signal_lossless_remux ORDER BY path")?;

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

    /// Get counts of lossless remux files grouped by file type.
    ///
    /// Returns (file_type, count) pairs sorted by count descending.
    pub fn get_lossless_remux_counts_by_type(&self) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT file_type, COUNT(*) as cnt FROM signal_lossless_remux GROUP BY file_type ORDER BY cnt DESC"
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

    /// Get all same-recording-different-release groups with deserialized data.
    pub fn get_same_recording_different_release_groups(
        &self,
    ) -> Result<Vec<(String, crate::meta::signals::data::SameRecordingDifferentReleaseData)>> {
        self.query_signal_key_blobs(
            "SELECT key, data FROM signal_same_recording_different_release ORDER BY key",
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
