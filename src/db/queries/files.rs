//! File table query operations.
//!
//! This module provides read-only queries for the new inode-based schema:
//! - `files`: All paths (files and directories) with mtime
//! - `audio_info`: Audio-specific metadata by inode
//! - `corpus_tags` / `inbox_tags`: Tags by inode
//!
//! Write operations go through `write_thread::SignalWriteSender`.

use anyhow::Result;
use rusqlite::params;
use std::collections::{HashMap, HashSet};

use super::Database;
use crate::db::types::{AudioFile, AudioInfo, AudioTag, FileEntry, Zone};

/// An audio file paired with its tag map (tag name → values).
/// Tag names are uppercased; values are collected into Vec since a
/// single tag name can have multiple values (e.g. multiple genres).
pub type AudioFileWithTags = (AudioFile, HashMap<String, Vec<String>>);

// ============================================================================
// Fingerprint Conversion Helpers
// ============================================================================

/// Convert BLOB bytes to fingerprint Vec<u32> (little-endian).
pub(crate) fn blob_to_fingerprint(blob: &[u8]) -> Vec<u32> {
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

// Column list for AudioFile queries (files JOIN audio_info).
// The fingerprint column is parameterized because some callers use NULL
// to avoid transferring ~7KB/file of fingerprint BLOBs needlessly.
fn audio_file_select(fp_col: &str) -> String {
    format!(
        r#"f.inode, f.zone, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos, f.file_size, f.scanned_at,
                a.file_type, a.duration_ms, a.bitrate_kbps, a.sample_rate, {fp_col}, a.needs_tag_flush"#
    )
}

/// The standard fingerprint column expression.
const FP_COL: &str = "a.fingerprint";

impl Database {
    // ========================================================================
    // File Entry Queries
    // ========================================================================

    /// Get a file entry by path for a specific source.
    ///
    /// Unlike `get_audio_file_by_path()`, this does NOT require audio_info.
    /// Use this for files that may not have been successfully indexed (e.g., corrupt files).
    pub fn get_file_entry_by_path(&self, path: &str, zone: &str) -> Result<Option<FileEntry>> {
        let result = self.conn.query_row(
            "SELECT inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at
             FROM files WHERE path = ?1 AND zone = ?2",
            params![path, zone],
            Self::row_to_file_entry,
        );

        match result {
            Ok(entry) => Ok(Some(entry)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get mtime info for files by inode (for incremental scanning).
    pub fn get_file_mtime_batch(
        &self,
        zone: Zone,
        inodes: &[i64],
    ) -> Result<HashMap<i64, (i64, i64)>> {
        if inodes.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = (0..inodes.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT inode, mtime_secs, mtime_nanos FROM files
             WHERE zone = ? AND inode IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;

        let zone_str = zone.as_str();
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&zone_str];
        for inode in inodes {
            params_vec.push(inode);
        }

        let mut result = HashMap::new();
        let rows = stmt.query_map(&params_vec[..], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?;

        for row in rows {
            let (inode, mtime_secs, mtime_nanos) = row?;
            result.insert(inode, (mtime_secs, mtime_nanos));
        }

        Ok(result)
    }

    /// Look up the zone and path for an inode in corpus/inbox zones.
    ///
    /// Used for cross-zone move detection: when a file is found in zone A
    /// but is indexed in zone B, this returns (zone_str, path) from zone B.
    ///
    /// Excludes Library zone — hard-linked deployments share inodes with
    /// corpus, so including library would produce ambiguous results.
    pub fn get_file_zone_and_path_by_inode(&self, inode: i64) -> Result<Option<(String, String)>> {
        let result = self.conn.query_row(
            "SELECT zone, path FROM files WHERE inode = ?1 AND zone != 'library'",
            [inode],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        );
        match result {
            Ok(pair) => Ok(Some(pair)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get paths for files by inode (for move detection).
    pub fn get_file_paths_batch(&self, zone: Zone, inodes: &[i64]) -> Result<HashMap<i64, String>> {
        if inodes.is_empty() {
            return Ok(HashMap::new());
        }

        let placeholders = (0..inodes.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT inode, path FROM files
             WHERE zone = ? AND inode IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;

        let zone_str = zone.as_str();
        let mut params_vec: Vec<&dyn rusqlite::ToSql> = vec![&zone_str];
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
        let sql = format!(
            "SELECT {} FROM files f JOIN audio_info a ON f.inode = a.inode WHERE f.path = ?1",
            audio_file_select(FP_COL),
        );
        let result = self.conn.query_row(&sql, params![path], Self::row_to_audio_file);

        match result {
            Ok(af) => Ok(Some(af)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get all audio files for a zone.
    ///
    /// Pass `with_fingerprints: false` if the caller does not need the fingerprint
    /// blob — this avoids transferring ~7KB per file (307MB total for the corpus)
    /// from SQLite into process memory needlessly.
    pub fn get_all_audio_files(
        &self,
        zone: Zone,
        with_fingerprints: bool,
    ) -> Result<Vec<AudioFile>> {
        let fp_col = if with_fingerprints { FP_COL } else { "NULL" };
        let sql = format!(
            "SELECT {} FROM files f JOIN audio_info a ON f.inode = a.inode \
             WHERE f.zone = ?1 AND f.is_dir = 0 ORDER BY f.path",
            audio_file_select(fp_col),
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let files = stmt.query_map(params![zone.as_str()], Self::row_to_audio_file)?;
        files.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get an audio file by inode from a specific zone.
    ///
    /// Zone is REQUIRED - querying by inode alone is incorrect because
    /// the primary key is (path, zone, inode). The same inode can exist
    /// in multiple zones (corpus and library for hard-linked files).
    pub fn get_audio_file_by_inode(&self, inode: i64, zone: Zone) -> Result<Option<AudioFile>> {
        let sql = format!(
            "SELECT {} FROM files f JOIN audio_info a ON f.inode = a.inode \
             WHERE f.inode = ?1 AND f.zone = ?2 AND f.is_dir = 0",
            audio_file_select(FP_COL),
        );
        let result = self.conn.query_row(&sql, params![inode, zone.as_str()], Self::row_to_audio_file);

        match result {
            Ok(af) => Ok(Some(af)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Get multiple audio files by their inodes from a specific zone.
    ///
    /// Zone is REQUIRED - querying by inode alone is incorrect because
    /// the primary key is (path, zone, inode). The same inode can exist
    /// in multiple zones (corpus and library for hard-linked files).
    pub fn get_audio_files_by_inodes(&self, inodes: &[i64], zone: Zone) -> Result<Vec<AudioFile>> {
        if inodes.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<String> = (1..=inodes.len()).map(|i| format!("?{}", i)).collect();

        let sql = format!(
            "SELECT {} FROM files f JOIN audio_info a ON f.inode = a.inode \
             WHERE f.inode IN ({}) AND f.zone = '{}' AND f.is_dir = 0 ORDER BY f.path",
            audio_file_select(FP_COL),
            placeholders.join(", "),
            zone.as_str(),
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

        let sql = format!(
            "SELECT {} FROM files f JOIN audio_info a ON f.inode = a.inode \
             WHERE f.zone = 'corpus' AND f.path LIKE ?1 ESCAPE '\\' AND f.is_dir = 0 ORDER BY f.path",
            audio_file_select(FP_COL),
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let files = stmt.query_map(params![pattern], Self::row_to_audio_file)?;
        files.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    /// Get all audio files in a directory tree for tag editing.
    pub fn get_audio_files_for_tag_editing(
        &self,
        dir_path: &std::path::Path,
    ) -> Result<Vec<AudioFile>> {
        let pattern = super::dir_like_pattern(dir_path);

        let sql = format!(
            "SELECT {} FROM files f JOIN audio_info a ON f.inode = a.inode \
             WHERE f.path LIKE ?1 ESCAPE '\\' AND f.is_dir = 0 ORDER BY f.path",
            audio_file_select(FP_COL),
        );
        let mut stmt = self.conn.prepare(&sql)?;

        let files = stmt
            .query_map(params![pattern], Self::row_to_audio_file)?
            .collect::<rusqlite::Result<Vec<_>>>()?;

        Ok(files)
    }

    /// Get inode groups with duplicates (multiple paths for same inode, corpus only).
    /// Returns: Vec<(inode, comma_separated_paths)>
    pub fn get_duplicate_inode_groups(&self) -> Result<Vec<(i64, String)>> {
        // Only detect duplicate inodes within corpus files
        let query = r#"SELECT inode, GROUP_CONCAT(path) as paths
                       FROM files
                       WHERE is_dir = 0 AND zone = 'corpus'
                       GROUP BY inode
                       HAVING COUNT(*) > 1"#;

        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map(params![], |row| {
            let inode: i64 = row.get(0)?;
            let paths_str: String = row.get(1)?;
            Ok((inode, paths_str))
        })?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Get audio files with their present tag names (for missing tag detection).
    #[allow(clippy::type_complexity)]
    fn get_audio_files_with_tag_presence_for_zone(
        &self,
        tag_table: &str,
        zone: &str,
    ) -> Result<Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)>>
    {
        let query = format!(
            r#"SELECT f.inode, f.path,
                   (SELECT tag_value FROM {tag_table} WHERE inode = f.inode AND UPPER(tag_name) = 'ALBUM' LIMIT 1) as album,
                   GROUP_CONCAT(UPPER(t.tag_name), ',') as present_tags,
                   (SELECT tag_value FROM {tag_table} WHERE inode = f.inode AND UPPER(tag_name) = 'ARTIST' LIMIT 1) as artist,
                   (SELECT tag_value FROM {tag_table} WHERE inode = f.inode AND UPPER(tag_name) = 'TITLE' LIMIT 1) as title
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            LEFT JOIN {tag_table} t ON f.inode = t.inode
            WHERE f.is_dir = 0 AND f.zone = ?1
            GROUP BY f.inode"#
        );
        let mut stmt = self.conn.prepare(&query)?;
        let rows = stmt.query_map(params![zone], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    /// Get audio files with their present tag names for any tagged zone.
    #[allow(clippy::type_complexity)]
    pub fn get_audio_files_with_tag_presence_for<Z: crate::zones::TaggedZone>(
        &self,
    ) -> Result<Vec<(i64, String, Option<String>, Option<String>, Option<String>, Option<String>)>>
    {
        self.get_audio_files_with_tag_presence_for_zone(Z::TAG_TABLE, Z::ZONE_STR)
    }

    /// Get albums that are compilations (more than one distinct ARTIST value).
    /// Returns the set of album names (as they appear in corpus_tags).
    pub fn get_compilation_albums(&self) -> Result<std::collections::HashSet<String>> {
        let query = r#"
            SELECT album FROM (
                SELECT ct_album.tag_value AS album,
                       COUNT(DISTINCT ct_artist.tag_value) AS artist_count
                FROM files f
                JOIN audio_info a ON f.inode = a.inode
                JOIN corpus_tags ct_album ON f.inode = ct_album.inode AND UPPER(ct_album.tag_name) = 'ALBUM'
                JOIN corpus_tags ct_artist ON f.inode = ct_artist.inode AND UPPER(ct_artist.tag_name) = 'ARTIST'
                WHERE f.is_dir = 0 AND f.zone = 'corpus'
                GROUP BY ct_album.tag_value
                HAVING artist_count > 1
            )
        "#;

        let mut stmt = self.conn.prepare(query)?;
        let albums = stmt
            .query_map(params![], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<std::collections::HashSet<String>>>()?;

        Ok(albums)
    }

    /// Get inodes that have any of the given tag values for a specific tag name.
    ///
    /// Get all audio file inodes mapped to their paths, zone-generic.
    pub fn get_all_inodes<Z: crate::zones::AudioZone>(&self) -> Result<HashMap<i64, String>> {
        self.get_all_inodes_for_zone(Z::ZONE_STR)
    }

    /// Get all audio file inodes mapped to their paths for a zone.
    fn get_all_inodes_for_zone(&self, zone: &str) -> Result<HashMap<i64, String>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode, path FROM files WHERE zone = ?1 AND is_dir = 0")?;
        let rows = stmt.query_map(params![zone], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut result = HashMap::new();
        for row in rows.flatten() {
            result.insert(row.0, row.1);
        }
        Ok(result)
    }

    /// Check whether an audio file has embedded pictures.
    pub fn get_has_pictures(&self, inode: i64) -> Result<bool> {
        let result = self.conn.query_row(
            "SELECT has_pictures FROM audio_info WHERE inode = ?1",
            params![inode],
            |row| row.get::<_, i32>(0),
        );
        match result {
            Ok(v) => Ok(v != 0),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    /// Get the file path for a single inode in the given zone.
    pub fn get_path_for_inode<Z: crate::zones::AudioZone>(&self, inode: i64) -> Result<Option<String>> {
        let sql = format!(
            "SELECT path FROM files WHERE inode = ?1 AND zone = '{}' AND is_dir = 0 LIMIT 1",
            Z::ZONE_STR
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let mut rows = stmt.query(params![inode])?;
        if let Some(row) = rows.next()? {
            Ok(Some(row.get(0)?))
        } else {
            Ok(None)
        }
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
            WHERE f.is_dir = 0 AND f.zone = 'corpus'
            ORDER BY f.inode, UPPER(ct.tag_name)
        "#;

        let mut stmt = self.conn.prepare(query)?;
        let rows = stmt.query_map(params![], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Get album/artist/album_artist data for all audio files (for inconsistent album artist detection, corpus only).
    /// Returns: Vec<(inode, album, artist, album_artist, catalog_number, isrc, year, flag_compilation)>
    #[allow(clippy::type_complexity)]
    pub fn get_album_artist_data(
        &self,
    ) -> Result<Vec<(i64, String, String, String, String, String, String, String)>> {
        use mm_utils::tag_names::compound_tag_sql_in;

        let album_artist_in = compound_tag_sql_in("ALBUM", "ARTIST");
        let catalog_number_in = compound_tag_sql_in("CATALOG", "NUMBER");

        // Only detect inconsistent album artist within corpus files
        // GROUP BY f.inode to collapse multi-value tags (e.g. a file with two
        // artist tags) into one row per inode, preventing cross-product blowup
        // that would make a single file appear as multiple "tracks".
        let sql = format!(
            r#"
            SELECT
                f.inode,
                COALESCE(MIN(album.tag_value), '') as album,
                COALESCE(MIN(artist.tag_value), '') as artist,
                COALESCE(MIN(album_artist.tag_value), '') as album_artist,
                COALESCE(MIN(catalog.tag_value), '') as catalog_number,
                COALESCE(MIN(isrc.tag_value), '') as isrc,
                COALESCE(MIN(year.tag_value), '') as year,
                COALESCE(MIN(flagcomp.tag_value), '') as flag_compilation
            FROM files f
            JOIN audio_info a ON f.inode = a.inode
            LEFT JOIN corpus_tags album
                ON f.inode = album.inode AND UPPER(album.tag_name) = 'ALBUM'
            LEFT JOIN corpus_tags artist
                ON f.inode = artist.inode AND UPPER(artist.tag_name) = 'ARTIST'
            LEFT JOIN corpus_tags album_artist
                ON f.inode = album_artist.inode AND UPPER(album_artist.tag_name) IN {album_artist_in}
            LEFT JOIN corpus_tags catalog
                ON f.inode = catalog.inode AND UPPER(catalog.tag_name) IN {catalog_number_in}
            LEFT JOIN corpus_tags isrc
                ON f.inode = isrc.inode AND UPPER(isrc.tag_name) = 'ISRC'
            LEFT JOIN corpus_tags year
                ON f.inode = year.inode AND UPPER(year.tag_name) = 'YEAR'
            LEFT JOIN corpus_tags flagcomp
                ON f.inode = flagcomp.inode AND UPPER(flagcomp.tag_name) = 'COMPILATION'
            WHERE f.is_dir = 0 AND f.zone = 'corpus' AND album.tag_value IS NOT NULL AND album.tag_value != ''
            GROUP BY f.inode
        "#
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
                row.get(5)?,
                row.get(6)?,
                row.get(7)?,
            ))
        })?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // ========================================================================
    // Tag Queries (Corpus)
    // ========================================================================

    /// Get all tags for an audio file in the given tagged zone.
    pub fn get_tags<Z: crate::zones::TaggedZone>(&self, inode: i64) -> Result<Vec<AudioTag>> {
        let sql = format!(
            "SELECT inode, tag_name, tag_value FROM {} WHERE inode = ?1 ORDER BY tag_name, tag_value",
            Z::TAG_TABLE
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let tags = stmt
            .query_map(params![inode], |row| {
                Ok(AudioTag {
                    inode: row.get(0)?,
                    tag_name: row.get(1)?,
                    tag_value: row.get(2)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(tags)
    }

    /// Get all tags for an audio file from the tag table appropriate for its zone.
    ///
    /// Corpus → corpus_tags, Inbox → inbox_tags, Library → error (no tags).
    pub fn get_tags_for_zone(&self, inode: i64, zone: Zone) -> Result<Vec<AudioTag>> {
        let table = zone
            .tag_table()
            .ok_or_else(|| anyhow::anyhow!("zone {:?} has no tag table", zone))?;
        let sql = format!(
            "SELECT inode, tag_name, tag_value FROM {} WHERE inode = ?1 ORDER BY tag_name, tag_value",
            table
        );
        let mut stmt = self.conn.prepare(&sql)?;
        let tags = stmt
            .query_map(params![inode], |row| {
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
    /// Tags are keyed by uppercase tag name; values are collected into Vec
    /// since a single tag name can have multiple values (e.g. multiple genres).
    ///
    /// Pass `with_fingerprints: false` if the caller does not need fingerprint
    /// data — see `get_all_audio_files` for the rationale.
    pub fn get_all_audio_files_with_tags(
        &self,
        zone: Zone,
        with_fingerprints: bool,
    ) -> Result<Vec<AudioFileWithTags>> {
        let files = self.get_all_audio_files(zone, with_fingerprints)?;
        let mut results = Vec::new();
        for file in files {
            let tags = self.get_tags::<crate::zones::CorpusZone>(file.inode())?;
            let mut tag_map: HashMap<String, Vec<String>> = HashMap::new();
            for t in tags {
                tag_map
                    .entry(t.tag_name.to_uppercase())
                    .or_default()
                    .push(t.tag_value);
            }
            results.push((file, tag_map));
        }
        Ok(results)
    }

    /// Get audio file count, optionally filtered by zone.
    pub fn get_audio_file_count(&self, zone: Option<&str>) -> Result<usize> {
        let count: i64 = if let Some(src) = zone {
            self.conn.query_row(
                "SELECT COUNT(*) FROM files f JOIN audio_info a ON f.inode = a.inode WHERE f.zone = ?1 AND f.is_dir = 0",
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

    /// Get image file count, optionally filtered by zone.
    pub fn get_image_file_count(&self, zone: Option<&str>) -> Result<usize> {
        let count: i64 = if let Some(src) = zone {
            self.conn.query_row(
                "SELECT COUNT(*) FROM files f JOIN image_info i ON f.inode = i.inode WHERE f.zone = ?1 AND f.is_dir = 0",
                params![src],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM files f JOIN image_info i ON f.inode = i.inode WHERE f.is_dir = 0",
                params![],
                |row| row.get(0),
            )?
        };
        Ok(count as usize)
    }

    // ========================================================================
    // Row Conversion Helpers
    // ========================================================================

    /// Convert a database row to a FileEntry struct.
    fn row_to_file_entry(row: &rusqlite::Row) -> rusqlite::Result<FileEntry> {
        let zone_str: String = row.get(1)?;
        let zone = Zone::from_str(&zone_str).unwrap_or(Zone::Corpus);
        let is_dir: i32 = row.get(3)?;

        Ok(FileEntry {
            inode: row.get(0)?,
            zone,
            path: row.get(2)?,
            _is_dir: is_dir != 0,
            _mtime_secs: row.get(4)?,
            _mtime_nanos: row.get(5)?,
            file_size: row.get(6)?,
            _scanned_at: row.get(7)?,
        })
    }

    /// Convert a database row to an AudioInfo struct.
    fn row_to_audio_info(row: &rusqlite::Row) -> rusqlite::Result<AudioInfo> {
        let fp_blob: Option<Vec<u8>> = row.get(5)?;
        let fingerprint = fp_blob.map(|blob| blob_to_fingerprint(&blob));
        let needs_tag_flush: i32 = row.get(6)?;

        Ok(AudioInfo {
            _inode: row.get(0)?,
            file_type: row.get(1)?,
            duration_ms: row.get(2)?,
            bitrate_kbps: row.get(3)?,
            sample_rate: row.get(4)?,
            fingerprint,
            _needs_tag_flush: needs_tag_flush != 0,
        })
    }

    /// Convert a joined row to an AudioFile struct.
    /// Expected columns: f.inode, f.zone, f.path, f.is_dir, f.mtime_secs, f.mtime_nanos,
    ///                   f.file_size, f.scanned_at, a.file_type, a.duration_ms, a.bitrate_kbps,
    ///                   a.sample_rate, a.fingerprint, a.needs_tag_flush
    fn row_to_audio_file(row: &rusqlite::Row) -> rusqlite::Result<AudioFile> {
        let zone_str: String = row.get(1)?;
        let zone = Zone::from_str(&zone_str).unwrap_or(Zone::Corpus);
        let is_dir: i32 = row.get(3)?;

        let fp_blob: Option<Vec<u8>> = row.get(12)?;
        let fingerprint = fp_blob.map(|blob| blob_to_fingerprint(&blob));
        let needs_tag_flush: i32 = row.get(13)?;

        Ok(AudioFile {
            entry: FileEntry {
                inode: row.get(0)?,
                zone,
                path: row.get(2)?,
                _is_dir: is_dir != 0,
                _mtime_secs: row.get(4)?,
                _mtime_nanos: row.get(5)?,
                file_size: row.get(6)?,
                _scanned_at: row.get(7)?,
            },
            audio: AudioInfo {
                _inode: row.get(0)?,
                file_type: row.get(8)?,
                duration_ms: row.get(9)?,
                bitrate_kbps: row.get(10)?,
                sample_rate: row.get(11)?,
                fingerprint,
                _needs_tag_flush: needs_tag_flush != 0,
            },
        })
    }

    // ========================================================================
    // Write Operations (for db_thread)
    // ========================================================================

    /// Drop file entry from index by inode and zone.
    pub fn drop_file_index_by_inode(
        &self,
        zone: &str,
        inode: i64,
        _witness: &impl crate::db::write_thread::SignalWitness,
    ) -> Result<usize> {
        let affected = self.conn.execute(
            "DELETE FROM files WHERE zone = ?1 AND inode = ?2",
            params![zone, inode],
        )?;
        Ok(affected)
    }

    /// Update file path for a given inode and zone.
    pub fn update_file_path(
        &self,
        zone: &str,
        inode: i64,
        new_path: &str,
        _witness: &impl crate::db::write_thread::SignalWitness,
    ) -> Result<()> {
        self.conn.execute(
            "UPDATE files SET path = ?1 WHERE zone = ?2 AND inode = ?3",
            params![new_path, zone, inode],
        )?;
        Ok(())
    }

    /// Update zone for a file (cross-zone move) and migrate tags between tag tables.
    ///
    /// Updates the zone column and copies tags from old tag table to new tag table,
    /// then deletes the old tag rows.
    pub fn update_file_zone(
        &self,
        old_zone: &str,
        inode: i64,
        new_zone: &str,
        _witness: &impl crate::db::write_thread::SignalWitness,
    ) -> Result<()> {
        use crate::db::types::Zone;

        // Update zone column
        self.conn.execute(
            "UPDATE files SET zone = ?1 WHERE zone = ?2 AND inode = ?3",
            params![new_zone, old_zone, inode],
        )?;

        // Migrate tags between tag tables
        let old_z = Zone::from_str(old_zone).unwrap_or(Zone::Corpus);
        let new_z = Zone::from_str(new_zone).unwrap_or(Zone::Corpus);
        let old_table = old_z.tag_table();
        let new_table = new_z.tag_table();

        if let (Some(src), Some(dst)) = (old_table, new_table) {
            if src != dst {
                // Copy tags from source to destination
                self.conn.execute(
                    &format!(
                        "INSERT OR REPLACE INTO {} (inode, tag_name, tag_value) \
                         SELECT inode, tag_name, tag_value FROM {} WHERE inode = ?1",
                        dst, src
                    ),
                    params![inode],
                )?;
                // Delete from source
                self.conn.execute(
                    &format!("DELETE FROM {} WHERE inode = ?1", src),
                    params![inode],
                )?;
            }
        }

        // Dirty inode marking for zone changes is handled by the post-execution
        // pipeline in apply_post_execution() — UpdateFilePath now includes TAGS
        // in its recomputation scope, triggering dirty marking for all affected inodes.

        Ok(())
    }

    // ========================================================================
    // Dirty Inode Queries (for incremental computations)
    // ========================================================================

    /// Check whether an inode has a pending_write marker.
    ///
    /// Returns true if this inode was recently written to by MM (via
    /// `write_file_tags`) and the write has not yet been reconciled by
    /// VerifyTags. Uses `dirty_inodes` with `computation_type = 'pending_write'`.
    pub fn is_pending_write(&self, inode: i64) -> Result<bool> {
        Ok(self
            .conn
            .query_row(
                "SELECT 1 FROM dirty_inodes WHERE inode = ?1 AND computation_type = 'pending_write'",
                params![inode],
                |_| Ok(()),
            )
            .is_ok())
    }

    /// Get all inodes marked dirty for a specific computation type.
    ///
    /// Used by per-inode computations (e.g., compound tag detection) to query
    /// only the inodes that need reprocessing instead of the entire corpus.
    pub fn get_dirty_inodes(&self, computation_type: &str) -> Result<Vec<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT inode FROM dirty_inodes WHERE computation_type = ?1")?;

        let rows = stmt.query_map(params![computation_type], |row| row.get(0))?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Get corpus inodes whose tag values contain a given separator string.
    ///
    /// Used by SeedCompoundTagDirtyInodes to find files affected by new
    /// tag_splitting separator config.
    pub fn get_corpus_inodes_with_tag_separator(
        &self,
        tag_name: &str,
        separator: &str,
    ) -> Result<Vec<i64>> {
        let like_pattern = format!("%{}%", separator);
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT ct.inode
             FROM corpus_tags ct
             JOIN files f ON ct.inode = f.inode
             WHERE f.zone = 'corpus'
               AND UPPER(ct.tag_name) = UPPER(?1)
               AND ct.tag_value LIKE ?2",
        )?;

        let rows = stmt.query_map(params![tag_name, like_pattern], |row| row.get(0))?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    // =========================================================================
    // Audio Sibling Queries
    // =========================================================================

    /// Find any single audio file in a corpus directory (for deriving album_dir from sibling tags).
    ///
    /// Used by `DeriveDeployHealthSignals` to determine the expected deploy path
    /// for sidecar images when the image itself has no audio_info row.
    pub fn get_any_audio_sibling_in_directory(
        &self,
        corpus_dir: &str,
    ) -> Result<Option<(i64, String)>> {
        let pattern = super::dir_like_pattern_str(corpus_dir);

        let result = self.conn.query_row(
            r#"SELECT f.inode, f.path FROM files f
               JOIN audio_info a ON f.inode = a.inode
               WHERE f.zone = 'corpus' AND f.path LIKE ?1 ESCAPE '\' AND f.is_dir = 0
               LIMIT 1"#,
            rusqlite::params![pattern],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        );

        match result {
            Ok(pair) => Ok(Some(pair)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    // =========================================================================
    // Image Info Queries
    // =========================================================================

    /// Check which inodes already have image_info rows (batch query).
    ///
    /// Used for image indexing freshness checks: if an image inode
    /// already has image_info AND its mtime hasn't changed, skip re-indexing.
    pub fn get_image_info_exists_batch(&self, inodes: &[i64]) -> Result<HashSet<i64>> {
        if inodes.is_empty() {
            return Ok(HashSet::new());
        }

        let placeholders = (0..inodes.len()).map(|_| "?").collect::<Vec<_>>().join(",");
        let query = format!(
            "SELECT inode FROM image_info WHERE inode IN ({})",
            placeholders
        );

        let mut stmt = self.conn.prepare(&query)?;

        let params_vec: Vec<&dyn rusqlite::ToSql> =
            inodes.iter().map(|i| i as &dyn rusqlite::ToSql).collect();

        let rows = stmt.query_map(&params_vec[..], |row| row.get::<_, i64>(0))?;

        Ok(rows.collect::<rusqlite::Result<HashSet<_>>>()?)
    }

    // =========================================================================
    // Image File Queries
    // =========================================================================

    /// Get all corpus image files with their inodes (batch query).
    ///
    /// Returns all image files in the corpus zone, avoiding per-directory LIKE scans.
    /// Used by sidecar deploy signal derivation for batch processing.
    pub fn get_all_corpus_images(&self) -> Result<Vec<CorpusImageEntry>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT f.inode, f.path, i.format, i.width, i.height, i.role
             FROM files f
             JOIN image_info i ON f.inode = i.inode
             WHERE f.zone = 'corpus' AND f.is_dir = 0",
        )?;

        let rows = stmt.query_map(params![], |row| {
            Ok(CorpusImageEntry {
                inode: row.get(0)?,
                path: row.get(1)?,
                format: row.get(2)?,
                width: row.get(3)?,
                height: row.get(4)?,
                role: row.get(5)?,
            })
        })?;

        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }
}

/// A corpus image file with its inode (for batch sidecar deploy processing).
#[derive(Debug, Clone)]
pub struct CorpusImageEntry {
    pub inode: i64,
    pub path: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
    pub role: String,
}
