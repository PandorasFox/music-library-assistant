//! Track table operations.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use std::path::PathBuf;

use super::Database;
use crate::corpus::db::types::{Track, TrackTag};

impl Database {
    // ========================================================================
    // Track Operations
    // ========================================================================

    pub fn insert_track(&self, track: &Track) -> Result<i64> {
        self.conn
            .execute(
                r#"
            INSERT OR REPLACE INTO tracks
            (path, source, inode, file_size, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
            "#,
                params![
                    &track.path,
                    &track.source,
                    &track.inode,
                    &track.file_size,
                    &track.file_type,
                    &track.duration_ms,
                    &track.bitrate_kbps,
                    &track.sample_rate,
                    &track.fingerprint,
                ],
            )
            .context("Failed to insert track")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Insert a track and its tags together.
    /// Returns the track ID.
    pub fn insert_track_with_tags(&self, track: &Track, tags: &[(String, String)]) -> Result<i64> {
        let track_id = self.insert_track(track)?;
        self.set_track_tags(track_id, tags)?;
        Ok(track_id)
    }

    /// Clear all tracks for a source, cascading to dependent tables.
    pub fn clear_source(&self, source: &str) -> Result<()> {
        // Get all track IDs for this source first
        let mut stmt = self.conn.prepare("SELECT id FROM tracks WHERE source = ?1")?;
        let track_ids: Vec<i64> = stmt
            .query_map(params![source], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        // Delete from dependent tables for each track
        for track_id in &track_ids {
            self.conn.execute(
                "DELETE FROM duplicate_group_members WHERE track_id = ?1",
                params![track_id],
            )?;
            self.conn.execute(
                "DELETE FROM tag_edit_history WHERE track_id = ?1",
                params![track_id],
            )?;
        }

        // Now delete the tracks
        self.conn
            .execute("DELETE FROM tracks WHERE source = ?1", params![source])
            .context("Failed to clear source")?;

        Ok(())
    }

    /// Delete a track from the index by its path.
    /// Also removes related entries from duplicate_group_members and tag_edit_history.
    /// Returns true if a track was deleted.
    pub fn delete_track_by_path(&self, path: &str) -> Result<bool> {
        // First, find the track ID
        let track_id: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM tracks WHERE path = ?1",
                params![path],
                |row| row.get(0),
            )
            .optional()
            .with_context(|| format!("Failed to find track by path: {}", path))?;

        let Some(track_id) = track_id else {
            return Ok(false); // Track not found
        };

        // Delete from dependent tables first (foreign key constraints)
        self.conn
            .execute(
                "DELETE FROM duplicate_group_members WHERE track_id = ?1",
                params![track_id],
            )
            .with_context(|| format!("Failed to delete duplicate group members for track: {}", path))?;

        self.conn
            .execute(
                "DELETE FROM tag_edit_history WHERE track_id = ?1",
                params![track_id],
            )
            .with_context(|| format!("Failed to delete tag edit history for track: {}", path))?;

        // Delete from health_issue_tracks before deleting the track
        self.conn
            .execute(
                "DELETE FROM health_issue_tracks WHERE track_id = ?1",
                params![track_id],
            )
            .with_context(|| format!("Failed to delete health issue tracks for: {}", path))?;

        // Now delete the track itself
        let deleted = self
            .conn
            .execute("DELETE FROM tracks WHERE id = ?1", params![track_id])
            .with_context(|| format!("Failed to delete track by path: {}", path))?;

        // Also clean up scan_state entry for this path
        // This ensures eyeballing won't report this as "missing" anymore
        self.conn
            .execute("DELETE FROM scan_state WHERE path = ?1", params![path])
            .with_context(|| format!("Failed to delete scan_state for: {}", path))?;

        Ok(deleted > 0)
    }

    /// Delete multiple tracks by path, returning count deleted.
    pub fn delete_tracks_by_paths(&self, paths: &[&str]) -> Result<usize> {
        let mut count = 0;
        for path in paths {
            if self.delete_track_by_path(path)? {
                count += 1;
            }
        }
        Ok(count)
    }

    /// Get all tracks for a specific source.
    pub fn get_all_tracks_for_source(&self, source: &str) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks WHERE source = ?1 ORDER BY path",
        )?;

        let tracks = stmt
            .query_map(params![source], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    pub fn get_track_count(&self, source: Option<&str>) -> Result<usize> {
        let count: i64 = if let Some(src) = source {
            self.conn.query_row(
                "SELECT COUNT(*) FROM tracks WHERE source = ?1",
                params![src],
                |row| row.get(0),
            )?
        } else {
            self.conn
                .query_row("SELECT COUNT(*) FROM tracks", params![], |row| row.get(0))?
        };
        Ok(count as usize)
    }

    pub fn log_scan(
        &self,
        source: &str,
        file_count: usize,
        started_at: &str,
        completed_at: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO scan_history (source, file_count, started_at, completed_at) VALUES (?1, ?2, ?3, ?4)",
            params![source, file_count as i64, started_at, completed_at],
        ).context("Failed to log scan")?;
        Ok(())
    }

    pub fn get_sources(&self) -> Result<Vec<String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT source FROM tracks ORDER BY source")?;
        let sources = stmt
            .query_map(params![], |row| row.get(0))?
            .collect::<Result<Vec<String>, _>>()?;
        Ok(sources)
    }

    pub fn get_all_tracks(&self, source: Option<&str>) -> Result<Vec<Track>> {
        let query = if source.is_some() {
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks WHERE source = ?1 ORDER BY path"
        } else {
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks ORDER BY path"
        };

        let mut stmt = self.conn.prepare(query)?;
        let tracks = if let Some(src) = source {
            stmt.query_map(params![src], Self::row_to_track)?
        } else {
            stmt.query_map(params![], Self::row_to_track)?
        };

        tracks.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all track inodes mapped to their corpus paths.
    ///
    /// Returns HashMap<inode, path> for all indexed tracks.
    /// Used for library health checks to match library files to corpus.
    pub fn get_all_track_inodes(&self) -> Result<std::collections::HashMap<i64, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT inode, path FROM tracks"
        )?;

        let mut result = std::collections::HashMap::new();
        let rows = stmt.query_map(params![], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (inode, path) = row?;
            result.insert(inode, path);
        }

        Ok(result)
    }

    /// Get a track by its ID.
    pub fn get_track_by_id(&self, track_id: i64) -> Result<Option<Track>> {
        let result = self.conn.query_row(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks WHERE id = ?1",
            params![track_id],
            Self::row_to_track,
        );

        match result {
            Ok(track) => Ok(Some(track)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get a track by its exact path.
    pub fn get_track_by_path(&self, path: &str) -> Result<Option<Track>> {
        let result = self.conn.query_row(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks WHERE path = ?1",
            params![path],
            Self::row_to_track,
        );

        match result {
            Ok(track) => Ok(Some(track)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all tracks with a specific fingerprint.
    pub fn get_tracks_by_fingerprint(&self, fingerprint: &str) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks
             WHERE fingerprint = ?1
             ORDER BY path",
        )?;

        let tracks = stmt
            .query_map(params![fingerprint], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    /// Get all tracks with a specific inode.
    pub fn get_tracks_by_inode(&self, inode: i64) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks
             WHERE inode = ?1
             ORDER BY path",
        )?;

        let tracks = stmt
            .query_map(params![inode], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    /// Get tracks by a list of IDs.
    pub fn get_tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        // Build IN clause with placeholders
        let placeholders: String = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks
             WHERE id IN ({})
             ORDER BY path",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;
        let tracks = stmt
            .query_map(rusqlite::params_from_iter(ids.iter()), Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    /// Get tracks by metadata (artist, album, title).
    /// Used for detecting metadata collisions.
    /// Joins with track_tags to match on tag values.
    pub fn get_tracks_by_metadata(
        &self,
        artist: &str,
        album: &str,
        title: &str,
    ) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, t.path, t.source, t.inode, t.file_size, t.file_type,
                    t.duration_ms, t.bitrate_kbps, t.sample_rate, t.fingerprint
             FROM tracks t
             LEFT JOIN track_tags ta ON t.id = ta.track_id AND ta.tag_name = 'artist'
             LEFT JOIN track_tags tb ON t.id = tb.track_id AND tb.tag_name = 'album'
             LEFT JOIN track_tags tt ON t.id = tt.track_id AND tt.tag_name = 'title'
             WHERE LOWER(COALESCE(ta.tag_value, '')) = LOWER(?1)
               AND LOWER(COALESCE(tb.tag_value, '')) = LOWER(?2)
               AND LOWER(COALESCE(tt.tag_value, '')) = LOWER(?3)
             ORDER BY t.path",
        )?;

        let tracks = stmt
            .query_map(params![artist, album, title], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    pub fn get_tracks_by_corpus_path_prefix(&self, path_prefix: &str) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks
             WHERE source = 'corpus' AND path LIKE ?1 || '%'
             ORDER BY path",
        )?;

        let tracks = stmt.query_map(params![path_prefix], Self::row_to_track)?;
        tracks.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn get_library_tracks_by_source(&self, source: &str) -> Result<Vec<Track>> {
        self.get_all_tracks(Some(source))
    }

    pub fn get_tracks_by_paths(
        &self,
        path_prefixes: &[PathBuf],
        source: &str,
    ) -> Result<Vec<Track>> {
        if path_prefixes.is_empty() {
            return Ok(Vec::new());
        }

        let conditions: Vec<String> = path_prefixes
            .iter()
            .enumerate()
            .map(|(i, _)| format!("path LIKE ?{}", i + 2))
            .collect();
        let where_clause = conditions.join(" OR ");

        let query = format!(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks
             WHERE source = ?1 AND fingerprint IS NOT NULL AND ({})
             ORDER BY path",
            where_clause
        );

        let mut stmt = self.conn.prepare(&query)?;

        let mut params: Vec<String> = vec![source.to_string()];
        for prefix in path_prefixes {
            let pattern = format!("{}%", prefix.to_string_lossy());
            params.push(pattern);
        }

        let param_refs: Vec<&dyn rusqlite::ToSql> =
            params.iter().map(|p| p as &dyn rusqlite::ToSql).collect();

        let tracks = stmt
            .query_map(&param_refs[..], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    /// Get all tracks with fingerprints in a specific directory (recursive).
    /// Used by sleuthing to find duplicates between selected directories.
    pub fn get_tracks_in_directory(&self, dir_path: &std::path::Path) -> Result<Vec<Track>> {
        let path_prefix = format!("{}%", dir_path.to_string_lossy());

        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks
             WHERE source = 'corpus' AND fingerprint IS NOT NULL AND path LIKE ?1
             ORDER BY path",
        )?;

        let tracks = stmt
            .query_map(params![path_prefix], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    /// Get all tracks in a directory tree for tag editing.
    /// Unlike `get_tracks_in_directory`, this does NOT filter by fingerprint or source.
    /// Used by corpus browser to enable editing all indexed tracks.
    pub fn get_tracks_for_tag_editing(&self, dir_path: &std::path::Path) -> Result<Vec<Track>> {
        // Add trailing separator to ensure we only match files within this directory
        let dir_str = dir_path.to_string_lossy();
        let path_prefix = if dir_str.ends_with(std::path::MAIN_SEPARATOR) {
            format!("{}%", dir_str)
        } else {
            format!("{}{}%", dir_str, std::path::MAIN_SEPARATOR)
        };

        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint
             FROM tracks
             WHERE path LIKE ?1
             ORDER BY path",
        )?;

        let tracks = stmt
            .query_map(params![path_prefix], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    // ========================================================================
    // Inode Analysis Operations
    // ========================================================================

    /// Find inodes that appear multiple times in corpus tracks.
    /// Returns a vector of (inode, paths) for each duplicate.
    /// Used by eyeballing to detect hard links or database inconsistencies.
    pub fn get_duplicate_inodes_in_corpus(&self) -> Result<Vec<(i64, Vec<String>)>> {
        // First, find inodes with count > 1
        let mut stmt = self.conn.prepare(
            "SELECT inode, COUNT(*) as cnt
             FROM tracks
             WHERE source = 'corpus'
             GROUP BY inode
             HAVING cnt > 1"
        )?;

        let duplicate_inodes: Vec<i64> = stmt
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        if duplicate_inodes.is_empty() {
            return Ok(Vec::new());
        }

        // Get all paths for each duplicate inode
        let mut result = Vec::new();
        for inode in duplicate_inodes {
            let mut path_stmt = self.conn.prepare(
                "SELECT path FROM tracks WHERE source = 'corpus' AND inode = ?1 ORDER BY path"
            )?;

            let paths: Vec<String> = path_stmt
                .query_map(params![inode], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            if paths.len() > 1 {
                result.push((inode, paths));
            }
        }

        Ok(result)
    }

    // ========================================================================
    // Tag Edit Operations
    // ========================================================================

    pub fn log_tag_edit(
        &self,
        track_id: i64,
        field_name: &str,
        old_value: Option<&str>,
        new_value: Option<&str>,
        session_id: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO tag_edit_history (track_id, field_name, old_value, new_value, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![track_id, field_name, old_value, new_value, session_id],
        ).context("Failed to log tag edit")?;
        Ok(())
    }

    // ========================================================================
    // Track Tags Operations
    // ========================================================================

    /// Get all tags for a track.
    pub fn get_track_tags(&self, track_id: i64) -> Result<Vec<TrackTag>> {
        let mut stmt = self.conn.prepare(
            "SELECT track_id, tag_name, tag_value FROM track_tags WHERE track_id = ?1 ORDER BY tag_name, tag_value"
        )?;
        let tags = stmt
            .query_map(params![track_id], |row| {
                Ok(TrackTag {
                    track_id: row.get(0)?,
                    tag_name: row.get(1)?,
                    tag_value: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(tags)
    }

    /// Search tracks by tag value (case-insensitive substring match).
    /// Returns all tracks that have a tag with the given name containing the value.
    pub fn search_tracks_by_tag(&self, tag_name: &str, value_pattern: &str) -> Result<Vec<(Track, std::collections::HashMap<String, String>)>> {
        let pattern = format!("%{}%", value_pattern.to_lowercase());
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, t.path, t.source, t.inode, t.file_size, t.file_type,
                    t.duration_ms, t.bitrate_kbps, t.sample_rate, t.fingerprint
             FROM tracks t
             JOIN track_tags tt ON t.id = tt.track_id
             WHERE LOWER(tt.tag_name) = LOWER(?1) AND LOWER(tt.tag_value) LIKE ?2
             ORDER BY t.path"
        )?;

        let track_rows = stmt.query_map(params![tag_name, pattern], |row| {
            Ok(Track {
                id: row.get(0)?,
                path: row.get(1)?,
                source: row.get(2)?,
                inode: row.get(3)?,
                file_size: row.get(4)?,
                file_type: row.get(5)?,
                duration_ms: row.get(6)?,
                bitrate_kbps: row.get(7)?,
                sample_rate: row.get(8)?,
                fingerprint: row.get(9)?,
            })
        })?;

        let mut results = Vec::new();
        for track_result in track_rows {
            let track = track_result?;
            if let Some(id) = track.id {
                // Get all tags for this track
                let tags = self.get_track_tags(id)?;
                let tag_map: std::collections::HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name, t.tag_value))
                    .collect();
                results.push((track, tag_map));
            }
        }
        Ok(results)
    }

    /// Get all tracks with their tags (for search functionality).
    pub fn get_all_tracks_with_tags(&self) -> Result<Vec<(Track, std::collections::HashMap<String, String>)>> {
        let tracks = self.get_all_tracks(None)?;
        let mut results = Vec::new();
        for track in tracks {
            if let Some(id) = track.id {
                let tags = self.get_track_tags(id)?;
                let tag_map: std::collections::HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name, t.tag_value))
                    .collect();
                results.push((track, tag_map));
            }
        }
        Ok(results)
    }

    /// Set all tags for a track (replaces existing tags).
    pub fn set_track_tags(&self, track_id: i64, tags: &[(String, String)]) -> Result<()> {
        // Delete existing tags
        self.conn.execute(
            "DELETE FROM track_tags WHERE track_id = ?1",
            params![track_id],
        )?;

        // Insert new tags
        for (name, value) in tags {
            if !value.is_empty() {
                self.conn.execute(
                    "INSERT INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                    params![track_id, name, value],
                )?;
            }
        }
        Ok(())
    }

    /// Update a single tag for a track (upsert semantics).
    pub fn update_track_tag(&self, track_id: i64, tag_name: &str, value: &str) -> Result<()> {
        // Delete existing value for this tag name
        self.conn.execute(
            "DELETE FROM track_tags WHERE track_id = ?1 AND tag_name = ?2",
            params![track_id, tag_name],
        )?;

        // Insert new value if non-empty
        if !value.is_empty() {
            self.conn.execute(
                "INSERT INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                params![track_id, tag_name, value],
            )?;
        }
        Ok(())
    }

    /// Delete a tag from a track.
    pub fn delete_track_tag(&self, track_id: i64, tag_name: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM track_tags WHERE track_id = ?1 AND tag_name = ?2",
            params![track_id, tag_name],
        )?;
        Ok(())
    }

    /// Get a specific tag value for a track (first value if multi-value).
    pub fn get_track_tag_value(&self, track_id: i64, tag_name: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT tag_value FROM track_tags WHERE track_id = ?1 AND tag_name = ?2 LIMIT 1",
            params![track_id, tag_name],
            |row| row.get(0),
        );
        match result {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // ========================================================================
    // Signal Resolution Operations
    // ========================================================================

    /// Update track path (for relocated files).
    /// Used by FileRelocated signal handler.
    pub fn update_track_path(&self, track_id: i64, new_path: &str) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tracks SET path = ?1 WHERE id = ?2",
                params![new_path, track_id],
            )
            .with_context(|| format!("Failed to update path for track {}", track_id))?;
        Ok(())
    }

    /// Delete track by ID.
    /// Cascades to dependent tables (duplicate_group_members, tag_edit_history, health_issue_tracks).
    /// Used by MissingFromDisk signal handler.
    pub fn delete_track(&self, track_id: i64) -> Result<bool> {
        // Delete from dependent tables first (foreign key constraints)
        self.conn
            .execute(
                "DELETE FROM duplicate_group_members WHERE track_id = ?1",
                params![track_id],
            )
            .with_context(|| format!("Failed to delete duplicate group members for track: {}", track_id))?;

        self.conn
            .execute(
                "DELETE FROM tag_edit_history WHERE track_id = ?1",
                params![track_id],
            )
            .with_context(|| format!("Failed to delete tag edit history for track: {}", track_id))?;

        self.conn
            .execute(
                "DELETE FROM health_issue_tracks WHERE track_id = ?1",
                params![track_id],
            )
            .with_context(|| format!("Failed to delete health issue tracks for track: {}", track_id))?;

        // Now delete the track itself
        let deleted = self
            .conn
            .execute("DELETE FROM tracks WHERE id = ?1", params![track_id])
            .with_context(|| format!("Failed to delete track: {}", track_id))?;

        Ok(deleted > 0)
    }

    /// Update track metadata (full replace for out-of-band changes).
    /// Preserves the track ID but replaces all other fields.
    /// Used by OutOfBandFileChange signal handler.
    /// Note: This only updates the tracks table, not track_tags.
    pub fn update_track_metadata(&self, track_id: i64, track: &Track) -> Result<()> {
        self.conn
            .execute(
                r#"
                UPDATE tracks SET
                    path = ?1, source = ?2, inode = ?3, file_size = ?4, file_type = ?5,
                    duration_ms = ?6, bitrate_kbps = ?7, sample_rate = ?8, fingerprint = ?9
                WHERE id = ?10
                "#,
                params![
                    &track.path,
                    &track.source,
                    &track.inode,
                    &track.file_size,
                    &track.file_type,
                    &track.duration_ms,
                    &track.bitrate_kbps,
                    &track.sample_rate,
                    &track.fingerprint,
                    track_id,
                ],
            )
            .with_context(|| format!("Failed to update metadata for track {}", track_id))?;
        Ok(())
    }

    /// Update track metadata and tags together.
    pub fn update_track_metadata_with_tags(
        &self,
        track_id: i64,
        track: &Track,
        tags: &[(String, String)],
    ) -> Result<()> {
        self.update_track_metadata(track_id, track)?;
        self.set_track_tags(track_id, tags)?;
        Ok(())
    }

    // ========================================================================
    // Row Conversion Helper
    // ========================================================================

    /// Convert a database row to a Track struct.
    /// Expected column order: id, path, source, inode, file_size, file_type,
    ///                        duration_ms, bitrate_kbps, sample_rate, fingerprint
    pub(super) fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<Track> {
        Ok(Track {
            id: Some(row.get(0)?),
            path: row.get(1)?,
            source: row.get(2)?,
            inode: row.get(3)?,
            file_size: row.get(4)?,
            file_type: row.get(5)?,
            duration_ms: row.get(6)?,
            bitrate_kbps: row.get(7)?,
            sample_rate: row.get(8)?,
            fingerprint: row.get(9)?,
        })
    }
}
