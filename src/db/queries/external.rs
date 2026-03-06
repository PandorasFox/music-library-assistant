//! External matching queries.
//!
//! Read-only queries for external matching tables (external_matches,
//! external_no_match, external_retry). Used by the fetch thread to
//! determine which inodes need lookup.

use anyhow::Result;
use rusqlite::params;
use std::path::Path;

use super::Database;

/// A row from external_matches joined with files, for signal derivation.
pub struct ExternalMatchRow {
    pub inode: i64,
    pub recording_id: String,
    pub confidence: f64,
    pub raw_response: Option<Vec<u8>>,
    pub path: String,
}

/// A row from release_packing_manifest.
pub struct PackingManifestRow {
    pub release_id: String,
    pub total_tracks: i32,
    pub release_title: String,
    pub release_artist: String,
}

/// An optimal packing score row (is_optimal=1 or per-inode query).
pub struct OptimalPackingScoreRow {
    pub release_id: String,
    pub inode: i64,
    pub recording_id: String,
    pub medium_pos: i32,
    pub track_pos: i32,
    pub track_title: String,
    pub medium_format: Option<String>,
    pub track_number: String,
    pub score: f64,
    pub score_breakdown: Vec<u8>,
}

/// A row from release_packing_candidates (Stage 1 → Stage 2 intermediate).
pub struct PackingCandidateRow {
    pub inode: i64,
    pub recording_id: String,
    pub confidence: f64,
    pub parent_dir: String,
    pub duration_ms: Option<i64>,
    pub tag_title: Option<String>,
    pub tag_artist: Option<String>,
    pub tag_album: Option<String>,
    pub tag_tracknumber: Option<String>,
    pub dir_file_count: i32,
}

/// A candidate inode for external lookup.
pub struct ExternalLookupCandidate {
    pub inode: i64,
    pub fingerprint: Vec<u32>,
    pub duration_secs: u32,
}

impl Database {
    /// Get external matches for corpus files, for signal derivation.
    ///
    /// Returns rows ordered by (inode, confidence DESC) so the caller can
    /// group by inode and take the first per group (highest confidence).
    pub fn get_external_matches_for_derivation(
        &self,
        source_key: i64,
    ) -> Result<Vec<ExternalMatchRow>> {
        let mut stmt = self.conn().prepare(
            r#"SELECT em.inode, em.recording_id, em.confidence, em.raw_response, f.path
               FROM external_matches em
               JOIN files f ON em.inode = f.inode
               WHERE f.zone = 'corpus' AND em.source = ?1
               ORDER BY em.inode, em.confidence DESC"#,
        )?;

        let rows = stmt.query_map(params![source_key], |row| {
            Ok(ExternalMatchRow {
                inode: row.get(0)?,
                recording_id: row.get(1)?,
                confidence: row.get(2)?,
                raw_response: row.get(3)?,
                path: row.get(4)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            if let Ok(r) = row {
                results.push(r);
            }
        }
        Ok(results)
    }

    /// Get external matches for corpus files, without raw_response blobs.
    ///
    /// Slim variant of `get_external_matches_for_derivation` for the release
    /// packing pipeline which never reads raw_response. Avoids loading kilobytes
    /// of AcoustID JSON per row.
    pub fn get_external_matches_slim(
        &self,
        source_key: i64,
    ) -> Result<Vec<ExternalMatchRow>> {
        let mut stmt = self.conn().prepare(
            r#"SELECT em.inode, em.recording_id, em.confidence, NULL, f.path
               FROM external_matches em
               JOIN files f ON em.inode = f.inode
               WHERE f.zone = 'corpus' AND em.source = ?1
               ORDER BY em.inode, em.confidence DESC"#,
        )?;

        let rows = stmt.query_map(params![source_key], |row| {
            Ok(ExternalMatchRow {
                inode: row.get(0)?,
                recording_id: row.get(1)?,
                confidence: row.get(2)?,
                raw_response: row.get(3)?,
                path: row.get(4)?,
            })
        })?;

        let mut results = Vec::new();
        for row in rows {
            if let Ok(r) = row {
                results.push(r);
            }
        }
        Ok(results)
    }

    /// Get corpus inodes needing external lookup for a given source.
    ///
    /// Returns inodes that:
    /// - Are corpus audio files with fingerprints
    /// - Have no existing match in external_matches for this source
    /// - Have no existing no-match in external_no_match for this source
    /// - Are in one of the eligible source directories
    ///
    /// Results are limited to `limit` rows for batch sizing.
    pub fn get_inodes_needing_lookup(
        &self,
        source_key: i64,
        eligible_dirs: &[&Path],
        limit: usize,
    ) -> Result<Vec<ExternalLookupCandidate>> {
        if eligible_dirs.is_empty() {
            return Ok(Vec::new());
        }

        // Build LIKE patterns for eligible directories
        let patterns: Vec<String> = eligible_dirs
            .iter()
            .map(|d| {
                let dir_str = format!("corpus/{}", d.display());
                let escaped = super::escape_like_wildcards(&dir_str);
                format!("{}/%", escaped)
            })
            .collect();

        // Build OR clause for directory matching
        let dir_conditions: Vec<String> = patterns
            .iter()
            .enumerate()
            .map(|(i, _)| format!("f.path LIKE ?{} ESCAPE '\\'", i + 3))
            .collect();
        let dir_clause = dir_conditions.join(" OR ");

        let sql = format!(
            r#"SELECT a.inode, a.fingerprint, a.duration_ms
               FROM audio_info a
               JOIN files f ON a.inode = f.inode
               WHERE f.zone = 'corpus'
                 AND a.fingerprint IS NOT NULL
                 AND a.duration_ms IS NOT NULL
                 AND ({dir_clause})
                 AND NOT EXISTS (
                     SELECT 1 FROM external_matches em
                     WHERE em.inode = a.inode AND em.source = ?1
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM external_no_match enm
                     WHERE enm.fingerprint = a.fingerprint AND enm.source = ?1
                 )
                 AND NOT EXISTS (
                     SELECT 1 FROM external_retry er
                     WHERE er.inode = a.inode AND er.source = ?1
                 )
               LIMIT ?2"#
        );

        let mut stmt = self.conn().prepare(&sql)?;

        // Bind source key and limit first, then directory patterns
        let mut param_values: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        param_values.push(Box::new(source_key));
        param_values.push(Box::new(limit as i64));
        for pattern in &patterns {
            param_values.push(Box::new(pattern.clone()));
        }

        let param_refs: Vec<&dyn rusqlite::types::ToSql> = param_values.iter().map(|p| p.as_ref()).collect();

        let rows = stmt.query_map(param_refs.as_slice(), |row| {
            let inode: i64 = row.get(0)?;
            let fp_blob: Vec<u8> = row.get(1)?;
            let duration_ms: i64 = row.get(2)?;

            // Convert BLOB back to Vec<u32> (little-endian)
            let fingerprint: Vec<u32> = fp_blob
                .chunks_exact(4)
                .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            Ok(ExternalLookupCandidate {
                inode,
                fingerprint,
                duration_secs: (duration_ms / 1000) as u32,
            })
        })?;

        let mut candidates = Vec::new();
        for row in rows {
            if let Ok(c) = row {
                candidates.push(c);
            }
        }

        Ok(candidates)
    }

    /// Get cached MusicBrainz recording JSON by recording ID.
    ///
    /// Returns (raw_json, fetched_at) if cached.
    pub fn get_mb_recording_cache(&self, recording_id: &str) -> Result<Option<(Vec<u8>, i64)>> {
        let mut stmt = self.conn().prepare(
            "SELECT raw_json, fetched_at FROM mb_recording_cache WHERE recording_id = ?1",
        )?;

        let result = stmt.query_row(params![recording_id], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        });

        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get cached MusicBrainz artist JSON by artist ID.
    ///
    /// Returns (raw_json, fetched_at) if cached.
    pub fn get_mb_artist_cache(&self, artist_id: &str) -> Result<Option<(Vec<u8>, i64)>> {
        let mut stmt = self.conn().prepare(
            "SELECT raw_json, fetched_at FROM mb_artist_cache WHERE artist_id = ?1",
        )?;

        let result = stmt.query_row(params![artist_id], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        });

        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get cached MusicBrainz release JSON by release ID.
    ///
    /// Returns (raw_json, fetched_at) if cached.
    pub fn get_mb_release_cache(&self, release_id: &str) -> Result<Option<(Vec<u8>, i64)>> {
        let mut stmt = self.conn().prepare(
            "SELECT raw_json, fetched_at FROM mb_release_cache WHERE release_id = ?1",
        )?;

        let result = stmt.query_row(params![release_id], |row| {
            Ok((row.get::<_, Vec<u8>>(0)?, row.get::<_, i64>(1)?))
        });

        match result {
            Ok(r) => Ok(Some(r)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Bulk-load cached MusicBrainz release JSON for a set of release IDs.
    ///
    /// Returns `Vec<(release_id, raw_json)>` for all release IDs that have cache entries.
    /// Uses chunked IN-clause queries for large ID sets.
    pub fn get_mb_release_cache_bulk(&self, release_ids: &[&str]) -> Result<Vec<(String, Vec<u8>)>> {
        if release_ids.is_empty() {
            return Ok(Vec::new());
        }

        let mut results = Vec::new();

        // Process in chunks to avoid SQLite variable limit (999)
        for chunk in release_ids.chunks(500) {
            let placeholders: Vec<&str> = chunk.iter().map(|_| "?").collect();
            let sql = format!(
                "SELECT release_id, raw_json FROM mb_release_cache WHERE release_id IN ({})",
                placeholders.join(", ")
            );
            let mut stmt = self.conn().prepare(&sql)?;
            let params: Vec<&dyn rusqlite::types::ToSql> = chunk.iter()
                .map(|id| id as &dyn rusqlite::types::ToSql)
                .collect();
            let rows = stmt.query_map(params.as_slice(), |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })?;
            for row in rows {
                results.push(row?);
            }
        }

        Ok(results)
    }

    /// Get known entity MBIDs that need fetching (no cache entry or stale).
    ///
    /// Queries `mb_known_entities` for entities of the given type that either
    /// have no corresponding cache row or a stale one. This is the generalized
    /// replacement for `get_recording_ids_needing_mb_fetch`.
    pub fn get_known_entities_needing_fetch(
        &self,
        entity_type: &str,
        ttl_secs: i64,
    ) -> Result<Vec<String>> {
        let stale_threshold = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - ttl_secs;

        let (cache_table, cache_id_col) = match entity_type {
            "recording" => ("mb_recording_cache", "recording_id"),
            "artist" => ("mb_artist_cache", "artist_id"),
            "release" => ("mb_release_cache", "release_id"),
            _ => return Ok(Vec::new()),
        };

        let sql = format!(
            r#"SELECT ke.mbid
               FROM mb_known_entities ke
               LEFT JOIN {cache_table} c ON ke.mbid = c.{cache_id_col}
               WHERE ke.entity_type = ?1
                 AND (c.{cache_id_col} IS NULL OR c.fetched_at < ?2)"#
        );

        let mut stmt = self.conn().prepare(&sql)?;
        let rows = stmt.query_map(params![entity_type, stale_threshold], |row| {
            row.get::<_, String>(0)
        })?;

        let mut results = Vec::new();
        for row in rows {
            if let Ok(id) = row {
                results.push(id);
            }
        }
        Ok(results)
    }

    /// Get recording IDs that need MB cache fetch.
    ///
    /// For each inode, returns up to `max_candidates` recording IDs by confidence
    /// that either have no MB cache entry or a stale one.
    pub fn get_recording_ids_needing_mb_fetch(
        &self,
        ttl_secs: i64,
        max_candidates: u32,
    ) -> Result<Vec<String>> {
        let stale_threshold = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            - ttl_secs;

        let sql = r#"
            WITH ranked AS (
                SELECT em.recording_id, em.inode, em.confidence,
                       ROW_NUMBER() OVER (PARTITION BY em.inode ORDER BY em.confidence DESC) as rn
                FROM external_matches em
                JOIN files f ON em.inode = f.inode
                WHERE f.zone = 'corpus' AND em.source = 1
            )
            SELECT DISTINCT r.recording_id
            FROM ranked r
            LEFT JOIN mb_recording_cache mrc ON r.recording_id = mrc.recording_id
            WHERE r.rn <= ?1
              AND (mrc.recording_id IS NULL OR mrc.fetched_at < ?2)
        "#;

        let mut stmt = self.conn().prepare(sql)?;
        let rows = stmt.query_map(params![max_candidates, stale_threshold], |row| {
            row.get::<_, String>(0)
        })?;

        let mut results = Vec::new();
        for row in rows {
            if let Ok(id) = row {
                results.push(id);
            }
        }
        Ok(results)
    }

    // =========================================================================
    // Release Packing Pipeline Queries
    // =========================================================================

    /// Get the full packing manifest (all releases in the current pipeline run).
    pub fn get_packing_manifest(&self) -> Result<Vec<PackingManifestRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT release_id, total_tracks, release_title, release_artist FROM release_packing_manifest",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(PackingManifestRow {
                release_id: row.get(0)?,
                total_tracks: row.get(1)?,
                release_title: row.get(2)?,
                release_artist: row.get(3)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get optimal (is_optimal=1) packing scores, grouped by release.
    pub fn get_optimal_packing_scores(&self) -> Result<Vec<OptimalPackingScoreRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT release_id, inode, recording_id, medium_pos, track_pos, track_title, \
             medium_format, track_number, score, score_breakdown \
             FROM release_packing_scores WHERE is_optimal = 1 \
             ORDER BY release_id, medium_pos, track_pos",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok(OptimalPackingScoreRow {
                release_id: row.get(0)?,
                inode: row.get(1)?,
                recording_id: row.get(2)?,
                medium_pos: row.get(3)?,
                track_pos: row.get(4)?,
                track_title: row.get(5)?,
                medium_format: row.get(6)?,
                track_number: row.get(7)?,
                score: row.get(8)?,
                score_breakdown: row.get(9)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }


    // =========================================================================
    // Signal Data Reading (for Release Packing Browser)
    // =========================================================================

    /// Read all ReleasePackingSignal rows with deserialized data.
    pub fn get_release_packing_signal_data(
        &self,
    ) -> Result<Vec<(i64, String, crate::meta::signals::data::ReleasePackingData)>> {
        let mut stmt = self.conn().prepare(
            "SELECT inode, path, data FROM signal_release_packing ORDER BY path",
        )?;
        let rows = stmt.query_map([], |row| {
            let inode: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((inode, path, blob))
        })?;
        let mut results = Vec::new();
        for row in rows {
            let (inode, path, blob) = row?;
            if let Ok(data) = bincode::deserialize(&blob) {
                results.push((inode, path, data));
            }
        }
        Ok(results)
    }

    /// Read all UnmatchedCorpusTrackSignal rows with deserialized data.
    pub fn get_unmatched_corpus_track_signal_data(
        &self,
    ) -> Result<Vec<(i64, String, crate::meta::signals::data::UnmatchedCorpusTrackData)>> {
        let mut stmt = self.conn().prepare(
            "SELECT inode, path, data FROM signal_unmatched_corpus_track ORDER BY path",
        )?;
        let rows = stmt.query_map([], |row| {
            let inode: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let blob: Vec<u8> = row.get(2)?;
            Ok((inode, path, blob))
        })?;
        let mut results = Vec::new();
        for row in rows {
            let (inode, path, blob) = row?;
            if let Ok(data) = bincode::deserialize(&blob) {
                results.push((inode, path, data));
            }
        }
        Ok(results)
    }

    /// Read all UnfilledReleaseSlotSignal rows with deserialized data.
    pub fn get_unfilled_release_slot_signal_data(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::UnfilledReleaseSlotData>> {
        let mut stmt = self.conn().prepare(
            "SELECT data FROM signal_unfilled_release_slot",
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        })?;
        let mut results = Vec::new();
        for row in rows {
            let blob = row?;
            if let Ok(data) = bincode::deserialize(&blob) {
                results.push(data);
            }
        }
        Ok(results)
    }

    /// Read all NearMissReleaseSignal rows with deserialized data.
    pub fn get_near_miss_release_signal_data(
        &self,
    ) -> Result<Vec<crate::meta::signals::data::NearMissReleaseData>> {
        let mut stmt = self.conn().prepare(
            "SELECT data FROM signal_near_miss_release",
        )?;
        let rows = stmt.query_map([], |row| {
            let blob: Vec<u8> = row.get(0)?;
            Ok(blob)
        })?;
        let mut results = Vec::new();
        for row in rows {
            let blob = row?;
            if let Ok(data) = bincode::deserialize(&blob) {
                results.push(data);
            }
        }
        Ok(results)
    }

    // =========================================================================
    // Release Packing Candidates Queries
    // =========================================================================

    /// Get all packing candidates for a specific release (indexed lookup).
    pub fn get_packing_candidates_for_release(
        &self,
        release_id: &str,
    ) -> Result<Vec<PackingCandidateRow>> {
        let mut stmt = self.conn().prepare(
            "SELECT inode, recording_id, confidence, parent_dir, duration_ms, \
             tag_title, tag_artist, tag_album, tag_tracknumber, dir_file_count \
             FROM release_packing_candidates WHERE release_id = ?1",
        )?;
        let rows = stmt.query_map(params![release_id], |row| {
            Ok(PackingCandidateRow {
                inode: row.get(0)?,
                recording_id: row.get(1)?,
                confidence: row.get(2)?,
                parent_dir: row.get(3)?,
                duration_ms: row.get(4)?,
                tag_title: row.get(5)?,
                tag_artist: row.get(6)?,
                tag_album: row.get(7)?,
                tag_tracknumber: row.get(8)?,
                dir_file_count: row.get(9)?,
            })
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get distinct (inode, path) pairs from the candidates table.
    ///
    /// Used by Stages 3-4 to get corpus paths without a full corpus scan.
    pub fn get_candidate_paths(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn().prepare(
            "SELECT DISTINCT inode, path FROM release_packing_candidates",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get distinct (inode, recording_id) pairs from the candidates table.
    ///
    /// Used by Stage 4 to build inode→recording_ids map without loading external_matches.
    pub fn get_candidate_inode_recordings(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn().prepare(
            "SELECT inode, recording_id FROM release_packing_candidates",
        )?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get retry candidates for a given source.
    ///
    /// Returns inodes that previously failed lookup and need retry.
    pub fn get_retry_candidates(
        &self,
        source_key: i64,
        limit: usize,
    ) -> Result<Vec<ExternalLookupCandidate>> {
        let mut stmt = self.conn().prepare(
            r#"SELECT er.inode, er.fingerprint, a.duration_ms
               FROM external_retry er
               JOIN audio_info a ON er.inode = a.inode
               WHERE er.source = ?1
                 AND a.duration_ms IS NOT NULL
               ORDER BY er.retry_count ASC, er.failed_at ASC
               LIMIT ?2"#,
        )?;

        let rows = stmt.query_map(params![source_key, limit as i64], |row| {
            let inode: i64 = row.get(0)?;
            let fp_blob: Vec<u8> = row.get(1)?;
            let duration_ms: i64 = row.get(2)?;

            let fingerprint: Vec<u32> = fp_blob
                .chunks_exact(4)
                .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
                .collect();

            Ok(ExternalLookupCandidate {
                inode,
                fingerprint,
                duration_secs: (duration_ms / 1000) as u32,
            })
        })?;

        let mut candidates = Vec::new();
        for row in rows {
            if let Ok(c) = row {
                candidates.push(c);
            }
        }

        Ok(candidates)
    }

    /// Count corpus files that have a non-null fingerprint in audio_info.
    pub fn count_fingerprinted_corpus_files(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT a.inode) FROM audio_info a
             JOIN files f ON a.inode = f.inode
             WHERE f.zone = 'corpus' AND a.fingerprint IS NOT NULL AND f.is_dir = 0",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Count distinct corpus inodes that have at least one external match.
    pub fn count_externally_matched_corpus_files(&self, source_key: i64) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT em.inode) FROM external_matches em
             JOIN files f ON em.inode = f.inode
             WHERE f.zone = 'corpus' AND em.source = ?1",
            [source_key],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Count distinct recording IDs across all external matches for corpus files.
    pub fn count_matched_recordings(&self, source_key: i64) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT em.recording_id) FROM external_matches em
             JOIN files f ON em.inode = f.inode
             WHERE f.zone = 'corpus' AND em.source = ?1",
            [source_key],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}
