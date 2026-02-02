//! File table query operations.
//!
//! This module provides read-only queries for the new inode-based schema:
//! - `files`: All paths (files and directories) with mtime
//! - `audio_info`: Audio-specific metadata by inode
//! - `corpus_tags` / `inbox_tags`: Tags by inode
//!
//! Write operations go through `db_thread::SignalWriteSender`.

use anyhow::Result;
use rusqlite::params;
use std::collections::HashMap;

use super::Database;
use crate::corpus::db::types::{AudioFile, AudioInfo, AudioTag, FileEntry, FileSource};

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
    // File Entry Queries
    // ========================================================================

    /// Get all files (not directories) for a given source.
    pub fn get_files_by_source(&self, source: FileSource) -> Result<Vec<FileEntry>> {
        let mut stmt = self.conn.prepare(
            "SELECT inode, source, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at
             FROM files
             WHERE source = ?1 AND is_dir = 0
             ORDER BY path"
        )?;

        let files = stmt.query_map(params![source.as_str()], Self::row_to_file_entry)?;
        files.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get a file entry by path.
    pub fn get_file_by_path(&self, path: &str) -> Result<Option<FileEntry>> {
        let result = self.conn.query_row(
            "SELECT inode, source, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at
             FROM files WHERE path = ?1",
            params![path],
            Self::row_to_file_entry,
        );

        match result {
            Ok(entry) => Ok(Some(entry)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get a file entry by path for a specific source.
    ///
    /// Unlike `get_audio_file_by_path()`, this does NOT require audio_info.
    /// Use this for files that may not have been successfully indexed (e.g., corrupt files).
    pub fn get_file_entry_by_path(&self, path: &str, source: &str) -> Result<Option<FileEntry>> {
        let result = self.conn.query_row(
            "SELECT inode, source, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at
             FROM files WHERE path = ?1 AND source = ?2",
            params![path, source],
            Self::row_to_file_entry,
        );

        match result {
            Ok(entry) => Ok(Some(entry)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get file count by source.
    pub fn get_file_count(&self, source: Option<FileSource>) -> Result<usize> {
        let count: i64 = if let Some(src) = source {
            self.conn.query_row(
                "SELECT COUNT(*) FROM files WHERE source = ?1 AND is_dir = 0",
                params![src.as_str()],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM files WHERE is_dir = 0",
                params![],
                |row| row.get(0),
            )?
        };
        Ok(count as usize)
    }

    /// Get all file inodes mapped to their paths for a source.
    pub fn get_all_file_inodes(&self, source: FileSource) -> Result<HashMap<i64, String>> {
        let mut stmt = self.conn.prepare(
            "SELECT inode, path FROM files WHERE source = ?1 AND is_dir = 0"
        )?;

        let mut result = HashMap::new();
        let rows = stmt.query_map(params![source.as_str()], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (inode, path) = row?;
            result.insert(inode, path);
        }

        Ok(result)
    }

    /// Get mtime info for files by inode (for incremental scanning).
    pub fn get_file_mtime_batch(
        &self,
        source: FileSource,
        inodes: &[i64],
    ) -> Result<HashMap<i64, (i64, i64)>> {
        if inodes.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = (0..inodes.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT inode, mtime_secs, mtime_nanos FROM files
             WHERE source = ? AND inode IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;

        let source_str = source.as_str();
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&source_str];
        for inode in inodes {
            params_vec.push(inode);
        }

        let mut result = HashMap::new();
        let rows = stmt.query_map(&params_vec[..], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?, row.get::<_, i64>(2)?))
        })?;

        for row in rows {
            let (inode, mtime_secs, mtime_nanos) = row?;
            result.insert(inode, (mtime_secs, mtime_nanos));
        }

        Ok(result)
    }

    /// Get paths for files by inode (for move detection).
    pub fn get_file_paths_batch(
        &self,
        source: FileSource,
        inodes: &[i64],
    ) -> Result<HashMap<i64, String>> {
        if inodes.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = (0..inodes.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT inode, path FROM files
             WHERE source = ? AND inode IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;

        let source_str = source.as_str();
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&source_str];
        for inode in inodes {
            params_vec.push(inode);
        }

        let mut result = HashMap::new();
        let rows = stmt.query_map(&params_vec[..], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;

        for row in rows {
            let (inode, path) = row?;
            result.insert(inode, path);
        }

        Ok(result)
    }

    // ========================================================================
    // Audio Info Queries
    // ========================================================================

    /// Get audio info by inode.
    pub fn get_audio_info(&self, inode: i64) -> Result<Option<AudioInfo>> {
        let result = self.conn.query_row(
            "SELECT inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, needs_tag_flush
             FROM audio_info WHERE inode = ?1",
            params![inode],
            Self::row_to_audio_info,
        );

        match result {
            Ok(info) => Ok(Some(info)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get audio file (combined file entry + audio info) by path.
    pub fn get_audio_file_by_path(&self, path: &str) -> Result<Option<AudioFile>> {
        let result = self.conn.query_row(
            r#"SELECT
                f.inode, f.source, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos, f.file_size, f.scanned_at,
                a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate, a.fingerprint, a.needs_tag_flush
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            WHERE f.path = ?1"#,
            params![path],
            Self::row_to_audio_file,
        );

        match result {
            Ok(af) => Ok(Some(af)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all audio files for a source.
    pub fn get_all_audio_files(&self, source: FileSource) -> Result<Vec<AudioFile>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT
                f.inode, f.source, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos, f.file_size, f.scanned_at,
                a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate, a.fingerprint, a.needs_tag_flush
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            WHERE f.source = ?1 AND f.is_dir = 0
            ORDER BY f.path"#
        )?;

        let files = stmt.query_map(params![source.as_str()], Self::row_to_audio_file)?;
        files.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get an audio file by inode.
    pub fn get_audio_file_by_inode(&self, inode: i64) -> Result<Option<AudioFile>> {
        let result = self.conn.query_row(
            r#"SELECT
                f.inode, f.source, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos, f.file_size, f.scanned_at,
                a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate, a.fingerprint, a.needs_tag_flush
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            WHERE f.inode = ?1 AND f.is_dir = 0
            LIMIT 1"#,
            params![inode],
            Self::row_to_audio_file,
        );

        match result {
            Ok(af) => Ok(Some(af)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get multiple audio files by their inodes.
    pub fn get_audio_files_by_inodes(&self, inodes: &[i64]) -> Result<Vec<AudioFile>> {
        if inodes.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (1..=inodes.len()).map(|i| format!("?{}", i)).collect();
        let sql = format!(
            r#"SELECT
                f.inode, f.source, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos, f.file_size, f.scanned_at,
                a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate, a.fingerprint, a.needs_tag_flush
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            WHERE f.inode IN ({}) AND f.is_dir = 0
            ORDER BY f.path"#,
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let params: Vec<&dyn rusqlite::types::ToSql> = inodes
            .iter()
            .map(|id| id as &dyn rusqlite::types::ToSql)
            .collect();

        let files = stmt
            .query_map(params.as_slice(), Self::row_to_audio_file)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(files)
    }

    /// Get all corpus audio files matching a path prefix (directory query).
    pub fn get_audio_files_by_path_prefix(&self, path_prefix: &str) -> Result<Vec<AudioFile>> {
        let pattern = super::dir_like_pattern_str(path_prefix);

        let mut stmt = self.conn.prepare(
            r#"SELECT
                f.inode, f.source, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos, f.file_size, f.scanned_at,
                a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate, a.fingerprint, a.needs_tag_flush
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            WHERE f.source = 'corpus' AND f.path LIKE ?1 ESCAPE '\\' AND f.is_dir = 0
            ORDER BY f.path"#,
        )?;

        let files = stmt.query_map(params![pattern], Self::row_to_audio_file)?;
        files.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all audio files in a directory tree for tag editing.
    pub fn get_audio_files_for_tag_editing(&self, dir_path: &std::path::Path) -> Result<Vec<AudioFile>> {
        let pattern = super::dir_like_pattern(dir_path);

        let mut stmt = self.conn.prepare(
            r#"SELECT
                f.inode, f.source, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos, f.file_size, f.scanned_at,
                a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate, a.fingerprint, a.needs_tag_flush
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            WHERE f.path LIKE ?1 ESCAPE '\\' AND f.is_dir = 0
            ORDER BY f.path"#,
        )?;

        let files = stmt
            .query_map(params![pattern], Self::row_to_audio_file)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(files)
    }

    /// Get count of audio files with fingerprints.
    pub fn get_fingerprinted_audio_count(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM audio_info WHERE fingerprint IS NOT NULL",
            params![],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get duplicate fingerprint groups (corpus files only).
    /// Returns: Vec<(fingerprint_blob, comma_separated_inodes)>
    pub fn get_duplicate_fingerprint_groups(&self) -> Result<Vec<(Vec<u8>, String)>> {
        // Join with files table to filter by source = 'corpus'
        // Library files should not be included in fingerprint overlap detection
        let query = "SELECT a.fingerprint, GROUP_CONCAT(a.inode) as inodes
                     FROM audio_info a
                     JOIN files f ON a.inode = f.inode
                     WHERE a.fingerprint IS NOT NULL AND f.source = 'corpus'
                     GROUP BY a.fingerprint
                     HAVING COUNT(*) > 1";

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

    /// Get inode groups with duplicates (multiple paths for same inode, corpus only).
    /// Returns: Vec<(inode, comma_separated_paths)>
    pub fn get_duplicate_inode_groups(&self) -> Result<Vec<(i64, String)>> {
        // Only detect duplicate inodes within corpus files
        let query = r#"SELECT inode, GROUP_CONCAT(path) as paths
                       FROM files
                       WHERE is_dir = 0 AND source = 'corpus'
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

    /// Get audio files with their present tag names (for missing tag detection, corpus only).
    /// Returns: Vec<(inode, path, album_or_none, comma_separated_lowercase_tags)>
    pub fn get_audio_files_with_tag_presence(&self) -> Result<Vec<(i64, String, Option<String>, Option<String>)>> {
        // Only check missing tags for corpus files
        let query = r#"
            SELECT f.inode, f.path,
                   (SELECT tag_value FROM corpus_tags WHERE inode = f.inode AND LOWER(tag_name) = 'album' LIMIT 1) as album,
                   GROUP_CONCAT(LOWER(ct.tag_name), ',') as present_tags
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            LEFT JOIN corpus_tags ct ON f.inode = ct.inode
            WHERE f.is_dir = 0 AND f.source = 'corpus'
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

    /// Get inodes that have any of the given tag values for a specific tag name.
    pub fn get_inodes_for_tag_values(&self, tag_name: &str, values: &[&str]) -> Result<Vec<i64>> {
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

    /// Get all corpus audio file inodes mapped to their paths.
    pub fn get_all_corpus_inodes(&self) -> Result<HashMap<i64, String>> {
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

    /// Get all tags ordered by inode and tag name (for metadata duplicate detection, corpus only).
    /// Returns: Vec<(inode, tag_name, tag_value)>
    pub fn get_all_tags_ordered(&self) -> Result<Vec<(i64, String, String)>> {
        // Only detect metadata duplicates within corpus files
        let query = r#"
            SELECT f.inode, ct.tag_name, ct.tag_value
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            JOIN corpus_tags ct ON f.inode = ct.inode
            WHERE f.is_dir = 0 AND f.source = 'corpus'
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

    /// Get album/artist/album_artist data for all audio files (for inconsistent album artist detection, corpus only).
    /// Returns: Vec<(inode, album, artist, album_artist, catalog_number, isrc)>
    pub fn get_album_artist_data(&self) -> Result<Vec<(i64, String, String, String, String, String)>> {
        // Only detect inconsistent album artist within corpus files
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
            WHERE f.is_dir = 0 AND f.source = 'corpus' AND album.tag_value IS NOT NULL AND album.tag_value != ''
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

    // ========================================================================
    // Tag Queries (Corpus)
    // ========================================================================

    /// Get all tags for an audio file (corpus).
    pub fn get_corpus_tags(&self, inode: i64) -> Result<Vec<AudioTag>> {
        let mut stmt = self.conn.prepare(
            "SELECT inode, tag_name, tag_value FROM corpus_tags WHERE inode = ?1 ORDER BY tag_name, tag_value"
        )?;

        let tags = stmt.query_map(params![inode], |row| {
            Ok(AudioTag {
                inode: row.get(0)?,
                tag_name: row.get(1)?,
                tag_value: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tags)
    }

    /// Get all tags for an audio file (inbox).
    pub fn get_inbox_tags(&self, inode: i64) -> Result<Vec<AudioTag>> {
        let mut stmt = self.conn.prepare(
            "SELECT inode, tag_name, tag_value FROM inbox_tags WHERE inode = ?1 ORDER BY tag_name, tag_value"
        )?;

        let tags = stmt.query_map(params![inode], |row| {
            Ok(AudioTag {
                inode: row.get(0)?,
                tag_name: row.get(1)?,
                tag_value: row.get(2)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(tags)
    }

    /// Get all audio files with their tags (for search functionality).
    pub fn get_all_audio_files_with_tags(&self, source: FileSource) -> Result<Vec<(AudioFile, HashMap<String, String>)>> {
        let files = self.get_all_audio_files(source)?;
        let mut results = Vec::new();
        for file in files {
            let tags = self.get_corpus_tags(file.inode())?;
            let tag_map: HashMap<String, String> = tags
                .into_iter()
                .map(|t| (t.tag_name, t.tag_value))
                .collect();
            results.push((file, tag_map));
        }
        Ok(results)
    }

    /// Get audio files by file types.
    /// Returns (inode, path, file_type) tuples.
    pub fn get_audio_files_by_types(&self, file_types: &[&str]) -> Result<Vec<(i64, String, String)>> {
        if file_types.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (0..file_types.len()).map(|i| format!("?{}", i + 1)).collect();
        let sql = format!(
            r#"SELECT f.inode, f.path, a.file_type
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.source = 'corpus' AND a.file_type IN ({})"#,
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let params: Vec<&dyn rusqlite::types::ToSql> = file_types
            .iter()
            .map(|ft| ft as &dyn rusqlite::types::ToSql)
            .collect();

        let rows = stmt.query_map(params.as_slice(), |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?, row.get::<_, String>(2)?))
        })?;

        let mut results = Vec::new();
        for row in rows {
            results.push(row?);
        }
        Ok(results)
    }

    /// Get file type breakdown for corpus files.
    pub fn get_audio_type_counts(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT a.file_type, COUNT(*) as count
               FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.source = 'corpus'
               GROUP BY a.file_type"#
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

    /// Get audio file count, optionally filtered by source.
    pub fn get_audio_file_count(&self, source: Option<&str>) -> Result<usize> {
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

    /// Get count of audio files that have fingerprints.
    pub fn get_fingerprinted_audio_file_count(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM audio_info WHERE fingerprint IS NOT NULL",
            params![],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    // ========================================================================
    // Row Conversion Helpers
    // ========================================================================

    /// Convert a database row to a FileEntry struct.
    fn row_to_file_entry(row: &rusqlite::Row) -> rusqlite::Result<FileEntry> {
        let source_str: String = row.get(1)?;
        let source = FileSource::from_str(&source_str).unwrap_or(FileSource::Corpus);
        let is_dir: i32 = row.get(3)?;

        Ok(FileEntry {
            inode: row.get(0)?,
            source,
            path: row.get(2)?,
            is_dir: is_dir != 0,
            mtime_secs: row.get(4)?,
            mtime_nanos: row.get(5)?,
            file_size: row.get(6)?,
            scanned_at: row.get(7)?,
        })
    }

    /// Convert a database row to an AudioInfo struct.
    fn row_to_audio_info(row: &rusqlite::Row) -> rusqlite::Result<AudioInfo> {
        let fp_blob: Option<Vec<u8>> = row.get(5)?;
        let fingerprint = fp_blob.map(|blob| blob_to_fingerprint(&blob));
        let needs_tag_flush: i32 = row.get(6)?;

        Ok(AudioInfo {
            inode: row.get(0)?,
            file_type: row.get(1)?,
            duration_ms: row.get(2)?,
            bitrate_kbps: row.get(3)?,
            sample_rate: row.get(4)?,
            fingerprint,
            needs_tag_flush: needs_tag_flush != 0,
        })
    }

    /// Convert a joined row to an AudioFile struct.
    /// Expected columns: f.inode, f.source, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos,
    ///                   f.file_size, f.scanned_at, a.file_type, a.duration_ms, a.bitrate_kbps,
    ///                   a.sample_rate, a.fingerprint, a.needs_tag_flush
    fn row_to_audio_file(row: &rusqlite::Row) -> rusqlite::Result<AudioFile> {
        let source_str: String = row.get(1)?;
        let source = FileSource::from_str(&source_str).unwrap_or(FileSource::Corpus);
        let is_dir: i32 = row.get(3)?;

        let fp_blob: Option<Vec<u8>> = row.get(12)?;
        let fingerprint = fp_blob.map(|blob| blob_to_fingerprint(&blob));
        let needs_tag_flush: i32 = row.get(13)?;

        Ok(AudioFile {
            entry: FileEntry {
                inode: row.get(0)?,
                source,
                path: row.get(2)?,
                is_dir: is_dir != 0,
                mtime_secs: row.get(4)?,
                mtime_nanos: row.get(5)?,
                file_size: row.get(6)?,
                scanned_at: row.get(7)?,
            },
            audio: AudioInfo {
                inode: row.get(0)?,
                file_type: row.get(8)?,
                duration_ms: row.get(9)?,
                bitrate_kbps: row.get(10)?,
                sample_rate: row.get(11)?,
                fingerprint,
                needs_tag_flush: needs_tag_flush != 0,
            },
        })
    }

    // ========================================================================
    // Write Operations (for db_thread)
    // ========================================================================

    /// Drop file entry from index by inode and source.
    pub fn drop_file_index_by_inode(
        &self,
        source: &str,
        inode: i64,
        _witness: &impl crate::db_thread::SignalWitness,
    ) -> Result<usize> {
        let affected = self.conn.execute(
            "DELETE FROM files WHERE source = ?1 AND inode = ?2",
            params![source, inode],
        )?;
        Ok(affected)
    }

    /// Update file path for a given inode and source.
    pub fn update_file_path(
        &self,
        source: &str,
        inode: i64,
        new_path: &str,
        _witness: &impl crate::db_thread::SignalWitness,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET path = ?1 WHERE source = ?2 AND inode = ?3",
            params![new_path, source, inode],
        )?;
        Ok(())
    }

    /// Cleanup stale file entries (inodes not in valid set).
    pub fn cleanup_stale_files(
        &self,
        source: &str,
        valid_inodes: &std::collections::HashSet<i64>,
        _witness: &impl crate::db_thread::SignalWitness,
    ) -> Result<usize> {
        if valid_inodes.is_empty() {
            // Delete all files for this source
            let affected = self.conn.execute(
                "DELETE FROM files WHERE source = ?1",
                params![source],
            )?;
            return Ok(affected);
        }

        // Build placeholders for IN clause
        let placeholders: Vec<String> = (0..valid_inodes.len())
            .map(|i| format!("?{}", i + 2))
            .collect();
        let sql = format!(
            "DELETE FROM files WHERE source = ?1 AND inode NOT IN ({})",
            placeholders.join(",")
        );

        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(source.to_string())];
        for inode in valid_inodes {
            params_vec.push(Box::new(*inode));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> = params_vec.iter().map(|b| b.as_ref()).collect();
        let affected = self.conn.execute(&sql, params_refs.as_slice())?;
        Ok(affected)
    }

    /// Update file mtime.
    pub fn update_file_mtime(
        &self,
        source: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        _witness: &impl crate::db_thread::SignalWitness,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET mtime_secs = ?1, mtime_nanos = ?2 WHERE source = ?3 AND inode = ?4",
            params![mtime_secs, mtime_nanos, source, inode],
        )?;
        Ok(())
    }
}
