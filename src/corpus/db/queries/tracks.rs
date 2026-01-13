//! Track table operations.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use std::path::PathBuf;

use super::Database;
use crate::corpus::db::types::Track;

impl Database {
    // ========================================================================
    // Track Operations
    // ========================================================================

    pub fn insert_track(&self, track: &Track) -> Result<i64> {
        self.conn
            .execute(
                r#"
            INSERT OR REPLACE INTO tracks
            (path, source, inode, file_size, file_type, artist, album, album_artist,
             title, track_number, genre, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc)
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)
            "#,
                params![
                    &track.path,
                    &track.source,
                    &track.inode,
                    &track.file_size,
                    &track.file_type,
                    &track.artist,
                    &track.album,
                    &track.album_artist,
                    &track.title,
                    &track.track_number,
                    &track.genre,
                    &track.duration_ms,
                    &track.bitrate_kbps,
                    &track.sample_rate,
                    &track.fingerprint,
                    &track.isrc,
                ],
            )
            .context("Failed to insert track")?;

        Ok(self.conn.last_insert_rowid())
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
        // This ensures heartbeat won't report this as "missing" anymore
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
            "SELECT id, path, source, inode, file_size, file_type, artist, album, album_artist,
                    title, track_number, genre, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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
            "SELECT id, path, source, inode, file_size, file_type, artist, album, album_artist,
                    title, track_number, genre, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
             FROM tracks WHERE source = ?1 ORDER BY path"
        } else {
            "SELECT id, path, source, inode, file_size, file_type, artist, album, album_artist,
                    title, track_number, genre, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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

    /// Get a track by its ID.
    pub fn get_track_by_id(&self, track_id: i64) -> Result<Option<Track>> {
        let result = self.conn.query_row(
            "SELECT id, path, source, inode, file_size, file_type,
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
             FROM tracks
             WHERE fingerprint = ?1
             ORDER BY path",
        )?;

        let tracks = stmt
            .query_map(params![fingerprint], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    /// Get tracks by metadata (artist, album, title).
    /// Used for detecting metadata collisions.
    pub fn get_tracks_by_metadata(
        &self,
        artist: &str,
        album: &str,
        title: &str,
    ) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type,
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
             FROM tracks
             WHERE LOWER(COALESCE(artist, '')) = LOWER(?1)
               AND LOWER(COALESCE(album, '')) = LOWER(?2)
               AND LOWER(COALESCE(title, '')) = LOWER(?3)
             ORDER BY path",
        )?;

        let tracks = stmt
            .query_map(params![artist, album, title], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    pub fn get_tracks_by_corpus_path_prefix(&self, path_prefix: &str) -> Result<Vec<Track>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, path, source, inode, file_size, file_type, artist, album, album_artist,
                    title, track_number, genre, duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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
                    artist, album, album_artist, title, track_number,
                    duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
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

    pub fn update_track_tag(&self, track_id: i64, field_name: &str, value: &str) -> Result<()> {
        let query = match field_name {
            "artist" => "UPDATE tracks SET artist = ?1 WHERE id = ?2",
            "album" => "UPDATE tracks SET album = ?1 WHERE id = ?2",
            "album_artist" => "UPDATE tracks SET album_artist = ?1 WHERE id = ?2",
            "title" => "UPDATE tracks SET title = ?1 WHERE id = ?2",
            "track_number" => {
                if let Ok(num) = value.parse::<i32>() {
                    self.conn
                        .execute(
                            "UPDATE tracks SET track_number = ?1 WHERE id = ?2",
                            params![num, track_id],
                        )
                        .context("Failed to update track_number")?;
                    return Ok(());
                } else {
                    return Ok(());
                }
            }
            "genre" => "UPDATE tracks SET genre = ?1 WHERE id = ?2",
            "isrc" => "UPDATE tracks SET isrc = ?1 WHERE id = ?2",
            _ => return Ok(()),
        };

        self.conn
            .execute(query, params![value, track_id])
            .with_context(|| format!("Failed to update {} for track {}", field_name, track_id))?;

        Ok(())
    }

    // ========================================================================
    // Row Conversion Helper
    // ========================================================================

    pub(super) fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<Track> {
        Ok(Track {
            id: Some(row.get(0)?),
            path: row.get(1)?,
            source: row.get(2)?,
            inode: row.get(3)?,
            file_size: row.get(4)?,
            file_type: row.get(5)?,
            artist: row.get(6)?,
            album: row.get(7)?,
            album_artist: row.get(8)?,
            title: row.get(9)?,
            track_number: row.get(10)?,
            genre: row.get(11)?,
            duration_ms: row.get(12)?,
            bitrate_kbps: row.get(13)?,
            sample_rate: row.get(14)?,
            fingerprint: row.get(15)?,
            isrc: row.get(16)?,
        })
    }
}
