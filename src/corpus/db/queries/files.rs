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

    /// Get count of audio files with fingerprints.
    pub fn get_fingerprinted_audio_count(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM audio_info WHERE fingerprint IS NOT NULL",
            params![],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get duplicate fingerprint groups.
    /// Returns: Vec<(fingerprint_blob, comma_separated_inodes)>
    pub fn get_duplicate_fingerprint_groups_new(&self) -> Result<Vec<(Vec<u8>, String)>> {
        let query = "SELECT fingerprint, GROUP_CONCAT(inode) as inodes
                     FROM audio_info
                     WHERE fingerprint IS NOT NULL
                     GROUP BY fingerprint
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
