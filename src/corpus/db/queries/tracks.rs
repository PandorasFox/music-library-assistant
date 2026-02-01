//! Track query operations - compatibility layer.
//!
//! These queries use the new inode-based schema (files, audio_info, corpus_tags)
//! but return the legacy Track/TrackTag types for UI compatibility.
//!
//! Note: `Track.id` is now set to `inode` (not a synthetic autoincrement).

use anyhow::Result;
use rusqlite::params;
use std::collections::HashMap;

use super::Database;
use crate::corpus::db::types::{Track, TrackTag};

// ============================================================================
// Fingerprint Conversion Helpers
// ============================================================================

/// Convert BLOB bytes to fingerprint Vec<u32> (little-endian).
fn blob_to_fingerprint(blob: &[u8]) -> Vec<u32> {
    blob.chunks_exact(4)
        .map(|chunk| u32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]))
        .collect()
}

/// Convert fingerprint Vec<u32> to text format (comma-separated) for signal keys.
pub fn fingerprint_to_text(fp: &[u32]) -> String {
    fp.iter()
        .map(|n| n.to_string())
        .collect::<Vec<_>>()
        .join(",")
}

impl Database {
    // ========================================================================
    // Core Track Queries (using new schema)
    // ========================================================================

    /// Get all tracks, optionally filtered by source.
    pub fn get_all_tracks(&self, source: Option<&str>) -> Result<Vec<Track>> {
        let query = if source.is_some() {
            r#"SELECT f.inode, f.path, f.source, f.inode, f.file_size,
                      a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate,
                      a.fingerprint, a.needs_tag_flush
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.source = ?1 AND f.is_dir = 0
               ORDER BY f.path"#
        } else {
            r#"SELECT f.inode, f.path, f.source, f.inode, f.file_size,
                      a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate,
                      a.fingerprint, a.needs_tag_flush
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.is_dir = 0
               ORDER BY f.path"#
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
    pub fn get_all_track_inodes(&self) -> Result<HashMap<i64, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT inode, path FROM files WHERE source = 'corpus' AND is_dir = 0"
        )?;

        let mut result = HashMap::new();
        let rows = stmt.query_map(params![], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (inode, path) = row?;
            result.insert(inode, path);
        }

        Ok(result)
    }

    /// Get a track by its ID (which is now inode).
    pub fn get_track_by_id(&self, track_id: i64) -> Result<Option<Track>> {
        // In new schema, track_id == inode
        let result = self.conn.query_row(
            r#"SELECT f.inode, f.path, f.source, f.inode, f.file_size,
                      a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate,
                      a.fingerprint, a.needs_tag_flush
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.inode = ?1 AND f.is_dir = 0
               LIMIT 1"#,
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
            r#"SELECT f.inode, f.path, f.source, f.inode, f.file_size,
                      a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate,
                      a.fingerprint, a.needs_tag_flush
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.path = ?1 AND f.is_dir = 0"#,
            params![path],
            Self::row_to_track,
        );

        match result {
            Ok(track) => Ok(Some(track)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get track count, optionally filtered by source.
    pub fn get_track_count(&self, source: Option<&str>) -> Result<usize> {
        let count: i64 = if let Some(src) = source {
            self.conn.query_row(
                "SELECT COUNT(*) FROM files f JOIN audio_info a ON f.inode = a.inode WHERE f.source = ?1 AND f.is_dir = 0",
                params![src],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM files f JOIN audio_info a ON f.inode = a.inode WHERE f.is_dir = 0",
                params![],
                |row| row.get(0),
            )?
        };
        Ok(count as usize)
    }

    /// Get count of tracks that have fingerprints.
    pub fn get_fingerprinted_track_count(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM audio_info WHERE fingerprint IS NOT NULL",
            params![],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get multiple tracks by their IDs (which are now inodes).
    pub fn get_tracks_by_ids(&self, ids: &[i64]) -> Result<Vec<Track>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (1..=ids.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            r#"SELECT f.inode, f.path, f.source, f.inode, f.file_size,
                      a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate,
                      a.fingerprint, a.needs_tag_flush
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.inode IN ({}) AND f.is_dir = 0
               ORDER BY f.path"#,
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let params: Vec<&dyn rusqlite::types::ToSql> = ids
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let tracks = stmt
            .query_map(params.as_slice(), Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    /// Get all corpus tracks matching a path prefix (directory query).
    pub fn get_tracks_by_corpus_path_prefix(&self, path_prefix: &str) -> Result<Vec<Track>> {
        let pattern = super::dir_like_pattern_str(path_prefix);

        let mut stmt = self.conn.prepare(
            r#"SELECT f.inode, f.path, f.source, f.inode, f.file_size,
                      a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate,
                      a.fingerprint, a.needs_tag_flush
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.source = 'corpus' AND f.path LIKE ?1 ESCAPE '\\' AND f.is_dir = 0
               ORDER BY f.path"#,
        )?;

        let tracks = stmt.query_map(params![pattern], Self::row_to_track)?;
        tracks.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all tracks in a directory tree for tag editing.
    pub fn get_tracks_for_tag_editing(&self, dir_path: &std::path::Path) -> Result<Vec<Track>> {
        let pattern = super::dir_like_pattern(dir_path);

        let mut stmt = self.conn.prepare(
            r#"SELECT f.inode, f.path, f.source, f.inode, f.file_size,
                      a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate,
                      a.fingerprint, a.needs_tag_flush
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.path LIKE ?1 ESCAPE '\\' AND f.is_dir = 0
               ORDER BY f.path"#,
        )?;

        let tracks = stmt
            .query_map(params![pattern], Self::row_to_track)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tracks)
    }

    // ========================================================================
    // Track Tags Queries (using corpus_tags)
    // ========================================================================

    /// Get all tags for a track (by inode).
    pub fn get_track_tags(&self, track_id: i64) -> Result<Vec<TrackTag>> {
        // track_id is now inode
        let mut stmt = self.conn.prepare(
            "SELECT inode, tag_name, tag_value FROM corpus_tags WHERE inode = ?1 ORDER BY tag_name, tag_value"
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

    /// Get all tracks with their tags.
    pub fn get_all_tracks_with_tags(&self) -> Result<Vec<(Track, HashMap<String, String>)>> {
        let tracks = self.get_all_tracks(None)?;
        let mut results = Vec::new();
        for track in tracks {
            if let Some(id) = track.id {
                let tags = self.get_track_tags(id)?;
                let tag_map: HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name, t.tag_value))
                    .collect();
                results.push((track, tag_map));
            }
        }
        Ok(results)
    }

    /// Get track IDs (inodes) that have any of the given tag values for a specific tag name.
    pub fn get_track_ids_for_tag_values(&self, tag_name: &str, values: &[&str]) -> Result<Vec<i64>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<&str> = values.iter().map(|_| "?").collect();
        let sql = format!(
            r#"SELECT DISTINCT ct.inode FROM corpus_tags ct
               INNER JOIN files f ON ct.inode = f.inode AND f.source = 'corpus'
               WHERE ct.tag_name = ?1 AND ct.tag_value IN ({})"#,
            placeholders.join(",")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(values.len() + 1);
        params.push(&tag_name);
        for v in values {
            params.push(v);
        }

        let ids = stmt
            .query_map(params.as_slice(), |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;

        Ok(ids)
    }

    // ========================================================================
    // Format Queries
    // ========================================================================

    /// Get track counts grouped by file type for corpus tracks.
    pub fn get_track_counts_by_file_type(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT a.file_type, COUNT(*) as count
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.source = 'corpus' AND f.is_dir = 0
               GROUP BY a.file_type"#,
        )?;

        let rows = stmt.query_map([], |row| {
            let file_type: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((file_type, count))
        })?;

        let mut counts = HashMap::new();
        for row in rows {
            let (file_type, count) = row?;
            counts.insert(file_type, count);
        }
        Ok(counts)
    }

    /// Get all corpus tracks with a given set of file types.
    /// Returns (inode, relative_path, file_type) tuples.
    pub fn get_tracks_by_file_types(&self, file_types: &[&str]) -> Result<Vec<(i64, String, String)>> {
        if file_types.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (0..file_types.len()).map(|i| format!("?{}", i + 1)).collect();
        let sql = format!(
            r#"SELECT f.inode, f.path, a.file_type
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.source = 'corpus' AND f.is_dir = 0 AND a.file_type IN ({})"#,
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let params: Vec<&dyn rusqlite::types::ToSql> = file_types
            .iter()
            .map(|ft| ft as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt.query_map(params.as_slice(), |row| {
            let id: i64 = row.get(0)?;
            let path: String = row.get(1)?;
            let file_type: String = row.get(2)?;
            Ok((id, path, file_type))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    // ========================================================================
    // Row Conversion Helper
    // ========================================================================

    /// Convert a database row to a Track struct.
    /// Expected column order: inode (as id), path, source, inode, file_size, file_type,
    ///                        duration_ms, bitrate_kbps, sample_rate, fingerprint, needs_tag_flush
    pub(super) fn row_to_track(row: &rusqlite::Row) -> rusqlite::Result<Track> {
        let fp_blob: Option<Vec<u8>> = row.get(9)?;
        let fingerprint = fp_blob.map(|blob| blob_to_fingerprint(&blob));
        let needs_tag_flush: i32 = row.get(10)?;

        Ok(Track {
            id: Some(row.get(0)?),  // inode as id
            path: row.get(1)?,
            source: row.get(2)?,
            inode: row.get(3)?,
            file_size: row.get(4)?,
            file_type: row.get(5)?,
            duration_ms: row.get(6)?,
            bitrate_kbps: row.get(7)?,
            sample_rate: row.get(8)?,
            fingerprint,
            needs_disk_flush: needs_tag_flush != 0,
        })
    }

    // ========================================================================
    // Aggregate Queries (for computations)
    // ========================================================================

    /// Get fingerprint groups with duplicates.
    /// Returns: Vec<(fingerprint_blob, comma_separated_inodes)>
    pub fn get_duplicate_fingerprint_groups(&self) -> Result<Vec<(Vec<u8>, String)>> {
        let query = r#"SELECT fingerprint, GROUP_CONCAT(inode) as inodes
                       FROM audio_info
                       WHERE fingerprint IS NOT NULL
                       GROUP BY fingerprint
                       HAVING COUNT(*) > 1"#;

        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map(params![], |row| {
            let fp_blob: Vec<u8> = row.get(0)?;
            let inodes_str: String = row.get(1)?;
            Ok((fp_blob, inodes_str))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get inode groups with duplicates (multiple paths for same inode).
    /// Returns: Vec<(inode, comma_separated_paths)>
    pub fn get_duplicate_inode_groups(&self) -> Result<Vec<(i64, String)>> {
        let query = r#"SELECT inode, GROUP_CONCAT(path) as paths
                       FROM files
                       WHERE is_dir = 0
                       GROUP BY inode
                       HAVING COUNT(*) > 1"#;

        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let paths_str: String = row.get(1)?;
            Ok((inode, paths_str))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get tracks with their present tag names (for missing tag detection).
    /// Returns: Vec<(inode, path, album_or_none, comma_separated_lowercase_tags)>
    pub fn get_tracks_with_tag_presence(&self) -> Result<Vec<(i64, String, Option<String>, Option<String>)>> {
        let query = r#"
            SELECT f.inode, f.path,
                   (SELECT tag_value FROM corpus_tags WHERE inode = f.inode AND LOWER(tag_name) = 'album' LIMIT 1) as album,
                   GROUP_CONCAT(LOWER(ct.tag_name), ',') as present_tags
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            LEFT JOIN corpus_tags ct ON f.inode = ct.inode
            WHERE f.is_dir = 0
            GROUP BY f.inode
        "#;

        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get all track tags ordered by track and tag name (for metadata duplicate detection).
    /// Returns: Vec<(inode, tag_name, tag_value)>
    pub fn get_all_track_tags_ordered(&self) -> Result<Vec<(i64, String, String)>> {
        let query = r#"
            SELECT f.inode, ct.tag_name, ct.tag_value
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            JOIN corpus_tags ct ON f.inode = ct.inode
            WHERE f.is_dir = 0
            ORDER BY f.inode, LOWER(ct.tag_name)
        "#;

        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get album/artist/album_artist data for all tracks (for inconsistent album artist detection).
    /// Returns: Vec<(inode, album, artist, album_artist, catalog_number, isrc)>
    pub fn get_album_artist_data(&self) -> Result<Vec<(i64, String, String, String, String, String)>> {
        let sql = r#"
            SELECT
                f.inode,
                COALESCE(album.tag_value, '') as album,
                COALESCE(artist.tag_value, '') as artist,
                COALESCE(album_artist.tag_value, '') as album_artist,
                COALESCE(catalog.tag_value, '') as catalog_number,
                COALESCE(isrc.tag_value, '') as isrc
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            LEFT JOIN corpus_tags album
                ON f.inode = album.inode AND LOWER(album.tag_name) = 'album'
            LEFT JOIN corpus_tags artist
                ON f.inode = artist.inode AND LOWER(artist.tag_name) = 'artist'
            LEFT JOIN corpus_tags album_artist
                ON f.inode = album_artist.inode AND LOWER(album_artist.tag_name) = 'album_artist'
            LEFT JOIN corpus_tags catalog
                ON f.inode = catalog.inode AND LOWER(catalog.tag_name) = 'catalognumber'
            LEFT JOIN corpus_tags isrc
                ON f.inode = isrc.inode AND LOWER(isrc.tag_name) = 'isrc'
            WHERE f.is_dir = 0 AND album.tag_value IS NOT NULL AND album.tag_value != ''
        "#;

        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
            ))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }
}
