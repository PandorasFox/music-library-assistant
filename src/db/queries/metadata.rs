//! OOB tag resolution queries and tag collision detection.

use anyhow::Result;
use rusqlite::params;

use super::Database;

/// A row from the album collision detection query:
/// (album, artist_context, isrc, catalog_number, year, date, mb_release_id).
pub type AlbumCollisionRow = (String, String, String, String, String, String, String);

impl Database {
    // ========================================================================
    // Tag Collision Detection Queries (for canonicalization signals)
    // ========================================================================

    /// Query album data with artist context and release identifiers for collision detection.
    /// Returns file-level data to allow filtering by ISRC/catalog_number.
    /// Only considers corpus files.
    /// Returns Vec of (album_value, artist_context, isrc, catalog_number).
    ///
    pub fn get_album_data_for_collision_detection(
        &self,
        mb_release_tag_name: &str,
    ) -> Result<Vec<AlbumCollisionRow>> {
        use mm_utils::tag_names::compound_tag_sql_in;

        let album_artist_in = compound_tag_sql_in("ALBUM", "ARTIST");
        let catalog_number_in = compound_tag_sql_in("CATALOG", "NUMBER");

        let sql = format!(
            r#"SELECT
                   album.tag_value as album,
                   COALESCE(album_artist.tag_value, artist.tag_value, '') as artist_context,
                   COALESCE(isrc.tag_value, '') as isrc,
                   COALESCE(catalog.tag_value, '') as catalog_number,
                   COALESCE(year.tag_value, '') as year,
                   COALESCE(date.tag_value, '') as date,
                   COALESCE(mb_release.tag_value, '') as mb_release_id
               FROM corpus_tags album
               INNER JOIN files f ON album.inode = f.inode AND f.zone = 'corpus'
               LEFT JOIN corpus_tags album_artist
                   ON album.inode = album_artist.inode
                   AND UPPER(album_artist.tag_name) IN {album_artist_in}
               LEFT JOIN corpus_tags artist
                   ON album.inode = artist.inode
                   AND UPPER(artist.tag_name) = 'ARTIST'
               LEFT JOIN corpus_tags isrc
                   ON album.inode = isrc.inode
                   AND UPPER(isrc.tag_name) = 'ISRC'
               LEFT JOIN corpus_tags catalog
                   ON album.inode = catalog.inode
                   AND UPPER(catalog.tag_name) IN {catalog_number_in}
               LEFT JOIN corpus_tags year
                   ON album.inode = year.inode
                   AND UPPER(year.tag_name) = 'YEAR'
               LEFT JOIN corpus_tags date
                   ON album.inode = date.inode
                   AND UPPER(date.tag_name) = 'DATE'
               LEFT JOIN corpus_tags mb_release
                   ON album.inode = mb_release.inode
                   AND UPPER(mb_release.tag_name) = UPPER(?1)
               WHERE UPPER(album.tag_name) = 'ALBUM'
                   AND album.tag_value IS NOT NULL
                   AND album.tag_value != ''"#
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let rows = stmt.query_map(params![mb_release_tag_name], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
                row.get::<_, String>(6)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get corpus inodes that have BOTH configured MB track + release tags present.
    ///
    /// A file is considered "fully MB-tagged" only when both tags are present,
    /// indicating it has been matched to a specific MusicBrainz release.
    pub fn get_mb_tagged_inodes(
        &self,
        mb_track_tag_name: &str,
        mb_release_tag_name: &str,
    ) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ct_track.inode
               FROM corpus_tags ct_track
               INNER JOIN files f ON ct_track.inode = f.inode AND f.zone = 'corpus'
               INNER JOIN corpus_tags ct_release
                   ON ct_track.inode = ct_release.inode
                   AND UPPER(ct_release.tag_name) = UPPER(?2)
                   AND ct_release.tag_value IS NOT NULL
                   AND ct_release.tag_value != ''
               WHERE UPPER(ct_track.tag_name) = UPPER(?1)
                   AND ct_track.tag_value IS NOT NULL
                   AND ct_track.tag_value != ''"#,
        )?;
        let rows = stmt.query_map(
            rusqlite::params![mb_track_tag_name, mb_release_tag_name],
            |row| row.get(0),
        )?;
        rows.collect::<rusqlite::Result<Vec<_>>>().map_err(Into::into)
    }

    // ========================================================================
    // Zone-Generic Tag Queries
    // ========================================================================

    /// Query distinct tag values with file counts for any tagged zone.
    /// Returns Vec of (tag_value, file_count).
    pub fn get_distinct_tag_values_for<Z: crate::zones::TaggedZone>(
        &self,
        tag_name: &str,
    ) -> Result<Vec<(String, usize)>> {
        let sql = format!(
            r#"SELECT t.tag_value, COUNT(DISTINCT t.inode) as file_count
               FROM {} t
               INNER JOIN files f ON t.inode = f.inode AND f.zone = ?1
               WHERE UPPER(t.tag_name) = UPPER(?2) AND t.tag_value IS NOT NULL AND t.tag_value != ''
               GROUP BY t.tag_value
               ORDER BY file_count DESC"#,
            Z::TAG_TABLE
        );

        let mut stmt = self.conn.prepare(&sql)?;
        let rows = stmt.query_map(params![Z::ZONE_STR, tag_name], |row| {
            let value: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((value, count as usize))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get inodes that have any of the given tag values for a specific tag name in any tagged zone.
    /// Uses normalized tag name matching (strips separators) for consistency.
    pub fn get_inodes_for_tag_values_in<Z: crate::zones::TaggedZone>(
        &self,
        tag_name: &str,
        values: &[&str],
    ) -> Result<Vec<i64>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }

        let normalized_tag_name = mm_utils::tag_names::normalize_tag_name(tag_name);

        let placeholders: Vec<&str> = values.iter().map(|_| "?").collect();
        let sql = format!(
            r#"SELECT DISTINCT t.inode FROM {} t
               INNER JOIN files f ON t.inode = f.inode AND f.zone = ?1
               WHERE REPLACE(REPLACE(REPLACE(REPLACE(UPPER(t.tag_name), '_', ''), '-', ''), ' ', ''), '.', '') = ?2
               AND t.tag_value IN ({})"#,
            Z::TAG_TABLE,
            placeholders.join(",")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let mut params: Vec<&dyn rusqlite::ToSql> = Vec::with_capacity(values.len() + 2);
        let zone_str = Z::ZONE_STR;
        params.push(&zone_str);
        params.push(&normalized_tag_name);
        for v in values {
            params.push(v);
        }

        let ids = stmt
            .query_map(params.as_slice(), |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<i64>>>()?;

        Ok(ids)
    }

    // ========================================================================
    // Album Value Queries (for embedded disc number detection)
    // ========================================================================

    /// Get ALBUM tag values with their inodes from the corpus.
    ///
    /// Returns Vec of (inode, album_value).
    /// Used by DetectDiscExtractions to find embedded disc numbers.
    pub fn get_album_values_with_inodes(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ct.inode, ct.tag_value FROM corpus_tags ct
               INNER JOIN files f ON ct.inode = f.inode AND f.zone = 'corpus'
               WHERE UPPER(ct.tag_name) = 'ALBUM' AND ct.tag_value IS NOT NULL AND ct.tag_value != ''"#,
        )?;

        let rows = stmt.query_map(params![], |row| Ok((row.get(0)?, row.get(1)?)))?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    /// Get TRACKNUMBER tag values with album/artist context from the corpus.
    ///
    /// Returns Vec of (inode, tracknumber, album, album_artist).
    /// Used by DetectDiscExtractions to find letter-prefixed track numbers.
    pub fn get_tracknumber_values_with_context(
        &self,
    ) -> Result<Vec<(i64, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ct.inode, ct.tag_value,
                      COALESCE((SELECT ct2.tag_value FROM corpus_tags ct2
                                WHERE ct2.inode = ct.inode AND UPPER(ct2.tag_name) = 'ALBUM'
                                LIMIT 1), ''),
                      COALESCE((SELECT ct3.tag_value FROM corpus_tags ct3
                                WHERE ct3.inode = ct.inode AND UPPER(ct3.tag_name) = 'ALBUM_ARTIST'
                                LIMIT 1),
                               (SELECT ct4.tag_value FROM corpus_tags ct4
                                WHERE ct4.inode = ct.inode AND UPPER(ct4.tag_name) = 'ALBUMARTIST'
                                LIMIT 1), '')
               FROM corpus_tags ct
               INNER JOIN files f ON ct.inode = f.inode AND f.zone = 'corpus'
               WHERE UPPER(ct.tag_name) = 'TRACKNUMBER' AND ct.tag_value IS NOT NULL AND ct.tag_value != ''"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })?;

        rows.collect::<rusqlite::Result<Vec<_>>>()
            .map_err(Into::into)
    }

    // ========================================================================
    // OOB Tag Resolution Queries
    // ========================================================================

    /// Get ALL OOB signal files classified into resolution buckets.
    ///
    /// Reads from typed tables and classifies by mismatch direction.
    /// - Bucket 0 (MtimeOnly): mtime_only_mismatch signal
    /// - Bucket 1 (DbOnly): all mismatches have disk_value NULL
    /// - Bucket 2 (DiskOnly): all mismatches have db_value NULL
    /// - Bucket 3 (Conflict): mismatches in both directions
    pub fn get_oob_files_bucketed(&self) -> Result<Vec<crate::meta::views::OobFile>> {
        use crate::meta::signals::data::TagMismatchEntry as TypedEntry;
        use crate::meta::views::{OobFile, ConflictBucket, TagMismatchEntry};

        /// Convert signal-level mismatches to view-level mismatches.
        fn to_view_mismatches(typed: Vec<TypedEntry>) -> Vec<TagMismatchEntry> {
            typed
                .into_iter()
                .map(|m| TagMismatchEntry {
                    field: m.tag_name,
                    db_value: m.db_value,
                    disk_value: m.disk_value,
                })
                .collect()
        }

        let mut files = Vec::new();

        // MtimeOnly signals — no tag mismatches
        {
            let mut stmt = self.conn.prepare(
                "SELECT s.inode, s.path FROM signal_mtime_only_mismatch s
                 INNER JOIN files f ON f.inode = s.inode AND f.zone = 'corpus'
                 ORDER BY s.path",
            )?;
            let rows = stmt.query_map(params![], |row| {
                Ok(OobFile {
                    inode: row.get(0)?,
                    path: row.get(1)?,
                    bucket: ConflictBucket::MtimeOnly,
                    mismatches: Vec::new(),
                })
            })?;
            for row in rows {
                files.push(row?);
            }
        }

        // OOB tag sync signals — carry mismatches, infer direction bucket
        {
            let mut stmt = self.conn.prepare(
                "SELECT s.inode, s.path, s.data FROM signal_oob_tag_sync s
                 INNER JOIN files f ON f.inode = s.inode AND f.zone = 'corpus'
                 ORDER BY s.path",
            )?;
            let rows = stmt.query_map(params![], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })?;
            for row in rows {
                let (inode, path, blob) = row?;
                let typed: Vec<TypedEntry> = bincode::deserialize(&blob).unwrap_or_default();
                let all_db_null = typed.iter().all(|m| m.db_value.is_none());
                let all_disk_null = typed.iter().all(|m| m.disk_value.is_none());
                let bucket = if all_db_null {
                    ConflictBucket::DiskOnly
                } else if all_disk_null {
                    ConflictBucket::DbOnly
                } else {
                    ConflictBucket::Conflict
                };
                files.push(OobFile {
                    inode,
                    path,
                    bucket,
                    mismatches: to_view_mismatches(typed),
                });
            }
        }

        // OOB tag conflict signals — always Conflict bucket, carry mismatches
        {
            let mut stmt = self.conn.prepare(
                "SELECT s.inode, s.path, s.data FROM signal_oob_tag_conflict s
                 INNER JOIN files f ON f.inode = s.inode AND f.zone = 'corpus'
                 ORDER BY s.path",
            )?;
            let rows = stmt.query_map(params![], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                ))
            })?;
            for row in rows {
                let (inode, path, blob) = row?;
                let typed: Vec<TypedEntry> = bincode::deserialize(&blob).unwrap_or_default();
                files.push(OobFile {
                    inode,
                    path,
                    bucket: ConflictBucket::Conflict,
                    mismatches: to_view_mismatches(typed),
                });
            }
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// Get files with MovedFile signals (same inode, different path).
    ///
    /// Reads from typed signal_moved_file table: inode, path (new), old_path, zones.
    pub fn get_moved_files(&self) -> Result<Vec<crate::meta::views::MovedFileInfo>> {
        use crate::meta::views::MovedFileInfo;

        let mut stmt = self.conn.prepare(
            "SELECT inode, old_path, path, old_zone, new_zone FROM signal_moved_file ORDER BY path",
        )?;

        let files: Vec<MovedFileInfo> = stmt
            .query_map(params![], |row| {
                Ok(MovedFileInfo {
                    inode: row.get(0)?,
                    old_path: row.get(1)?,
                    new_path: row.get(2)?,
                    old_zone: row.get(3)?,
                    new_zone: row.get(4)?,
                })
            })?
            .filter_map(|r| r.ok())
            .collect();

        Ok(files)
    }

}
