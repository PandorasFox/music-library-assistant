//! OOB tag resolution queries and tag collision detection.

use anyhow::Result;
use rusqlite::params;

use super::Database;

impl Database {
    // ========================================================================
    // Tag Collision Detection Queries (for canonicalization signals)
    // ========================================================================

    /// Query distinct tag values with file counts from corpus_tags table.
    /// Only considers corpus files.
    /// Returns Vec of (tag_value, file_count).
    pub fn get_distinct_tag_values(&self, tag_name: &str) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT ct.tag_value, COUNT(DISTINCT ct.inode) as file_count
               FROM corpus_tags ct
               INNER JOIN files f ON ct.inode = f.inode AND f.zone = 'corpus'
               WHERE UPPER(ct.tag_name) = UPPER(?1) AND ct.tag_value IS NOT NULL AND ct.tag_value != ''
               GROUP BY ct.tag_value
               ORDER BY file_count DESC"#,
        )?;

        let rows = stmt.query_map(params![tag_name], |row| {
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

    /// Query album data with artist context and release identifiers for collision detection.
    /// Returns file-level data to allow filtering by ISRC/catalog_number.
    /// Only considers corpus files.
    /// Returns Vec of (album_value, artist_context, isrc, catalog_number).
    ///
    /// TODO: Expand query to include additional release identifiers when we need them:
    /// - MUSICBRAINZ_ALBUMID, MUSICBRAINZ_RELEASEGROUPID
    /// - DISCOGS_RELEASE_ID
    /// - BARCODE
    pub fn get_album_data_for_collision_detection(
        &self,
    ) -> Result<Vec<(String, String, String, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   album.tag_value as album,
                   COALESCE(album_artist.tag_value, artist.tag_value, '') as artist_context,
                   COALESCE(isrc.tag_value, '') as isrc,
                   COALESCE(catalog.tag_value, '') as catalog_number,
                   COALESCE(year.tag_value, '') as year,
                   COALESCE(date.tag_value, '') as date
               FROM corpus_tags album
               INNER JOIN files f ON album.inode = f.inode AND f.zone = 'corpus'
               LEFT JOIN corpus_tags album_artist
                   ON album.inode = album_artist.inode
                   AND UPPER(album_artist.tag_name) = 'ALBUM_ARTIST'
               LEFT JOIN corpus_tags artist
                   ON album.inode = artist.inode
                   AND UPPER(artist.tag_name) = 'ARTIST'
               LEFT JOIN corpus_tags isrc
                   ON album.inode = isrc.inode
                   AND UPPER(isrc.tag_name) = 'ISRC'
               LEFT JOIN corpus_tags catalog
                   ON album.inode = catalog.inode
                   AND UPPER(catalog.tag_name) = 'CATALOG_NUMBER'
               LEFT JOIN corpus_tags year
                   ON album.inode = year.inode
                   AND UPPER(year.tag_name) = 'YEAR'
               LEFT JOIN corpus_tags date
                   ON album.inode = date.inode
                   AND UPPER(date.tag_name) = 'DATE'
               WHERE UPPER(album.tag_name) = 'ALBUM'
                   AND album.tag_value IS NOT NULL
                   AND album.tag_value != ''"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, String>(5)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ========================================================================
    // Inbox Tag Queries (for inbox tag canonicity detection)
    // ========================================================================

    /// Query distinct tag values with file counts from inbox_tags table.
    /// Only considers inbox files.
    /// Returns Vec of (tag_value, file_count).
    pub fn get_distinct_inbox_tag_values(&self, tag_name: &str) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT it.tag_value, COUNT(DISTINCT it.inode) as file_count
               FROM inbox_tags it
               INNER JOIN files f ON it.inode = f.inode AND f.zone = 'inbox'
               WHERE UPPER(it.tag_name) = UPPER(?1) AND it.tag_value IS NOT NULL AND it.tag_value != ''
               GROUP BY it.tag_value
               ORDER BY file_count DESC"#,
        )?;

        let rows = stmt.query_map(params![tag_name], |row| {
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

    /// Get inbox inodes that have any of the given tag values for a specific tag name.
    pub fn get_inbox_inodes_for_tag_values(&self, tag_name: &str, values: &[&str]) -> Result<Vec<i64>> {
        if values.is_empty() {
            return Ok(Vec::new());
        }

        let placeholders: Vec<&str> = values.iter().map(|_| "?").collect();
        let sql = format!(
            r#"SELECT DISTINCT it.inode FROM inbox_tags it
               INNER JOIN files f ON it.inode = f.inode AND f.zone = 'inbox'
               WHERE UPPER(it.tag_name) = UPPER(?1) AND it.tag_value IN ({})"#,
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
    // OOB Tag Resolution Queries
    // ========================================================================

    /// Get files with purely one-directional tag mismatches (sync-eligible).
    ///
    /// Reads from typed signal_oob_tag_sync table with bincode BLOB for mismatches.
    pub fn get_oob_sync_files(&self) -> Result<Vec<crate::corpus::db::types::OobSyncFile>> {
        use crate::corpus::db::types::{OobSyncDirection, OobSyncFile, TagMismatchEntry};
        use crate::meta::signals::data::TagMismatchEntry as TypedEntry;

        let mut stmt = self.conn.prepare(
            "SELECT s.inode, s.path, s.data
             FROM signal_oob_tag_sync s
             INNER JOIN files f ON f.inode = s.inode AND f.zone = 'corpus'"
        )?;

        let mut files = Vec::new();

        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })?;

        for row in rows {
            let (inode, path, blob) = row?;

            let typed_mismatches: Vec<TypedEntry> =
                bincode::deserialize(&blob).unwrap_or_default();

            if typed_mismatches.is_empty() {
                continue;
            }

            let mismatches: Vec<TagMismatchEntry> = typed_mismatches.into_iter().map(|m| {
                TagMismatchEntry {
                    field: m.tag_name,
                    db_value: m.db_value,
                    disk_value: m.disk_value,
                    _db_values: Vec::new(),
                    _disk_values: Vec::new(),
                }
            }).collect();

            // Determine direction: all db NULL → DiskToIndex, all disk NULL → IndexToDisk
            let all_db_null = mismatches.iter().all(|m| m.db_value.is_none());
            let all_disk_null = mismatches.iter().all(|m| m.disk_value.is_none());

            let direction = if all_db_null {
                OobSyncDirection::DiskToIndex
            } else if all_disk_null {
                OobSyncDirection::IndexToDisk
            } else {
                // Mixed — shouldn't happen for sync signals, skip
                continue;
            };

            files.push(OobSyncFile {
                inode,
                path,
                direction,
                mismatches,
            });
        }

        Ok(files)
    }

    /// Get ALL OOB signal files classified into resolution buckets.
    ///
    /// Reads from typed tables and classifies by mismatch direction.
    /// - Bucket 0 (MtimeOnly): mtime_only_mismatch signal
    /// - Bucket 1 (DbOnly): all mismatches have disk_value NULL
    /// - Bucket 2 (DiskOnly): all mismatches have db_value NULL
    /// - Bucket 3 (Conflict): mismatches in both directions
    pub fn get_oob_files_bucketed(&self) -> Result<Vec<crate::corpus::db::types::BucketedOobFile>> {
        use crate::corpus::db::types::{BucketedOobFile, ConflictBucket};
        use crate::meta::signals::data::TagMismatchEntry as TypedEntry;

        let mut files = Vec::new();

        // MtimeOnly signals
        {
            let mut stmt = self.conn.prepare(
                "SELECT s.inode, s.path FROM signal_mtime_only_mismatch s
                 INNER JOIN files f ON f.inode = s.inode AND f.zone = 'corpus'
                 ORDER BY s.path"
            )?;
            let rows = stmt.query_map(params![], |row| {
                Ok(BucketedOobFile {
                    inode: row.get(0)?,
                    path: row.get(1)?,
                    bucket: ConflictBucket::MtimeOnly,
                })
            })?;
            for row in rows { files.push(row?); }
        }

        // OOB tag sync signals — infer direction from mismatches
        {
            let mut stmt = self.conn.prepare(
                "SELECT s.inode, s.path, s.data FROM signal_oob_tag_sync s
                 INNER JOIN files f ON f.inode = s.inode AND f.zone = 'corpus'
                 ORDER BY s.path"
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
                let mismatches: Vec<TypedEntry> = bincode::deserialize(&blob).unwrap_or_default();
                let all_db_null = mismatches.iter().all(|m| m.db_value.is_none());
                let all_disk_null = mismatches.iter().all(|m| m.disk_value.is_none());
                let bucket = if all_db_null { ConflictBucket::DiskOnly }
                    else if all_disk_null { ConflictBucket::DbOnly }
                    else { ConflictBucket::Conflict };
                files.push(BucketedOobFile { inode, path, bucket });
            }
        }

        // OOB tag conflict signals — always Conflict bucket
        {
            let mut stmt = self.conn.prepare(
                "SELECT s.inode, s.path FROM signal_oob_tag_conflict s
                 INNER JOIN files f ON f.inode = s.inode AND f.zone = 'corpus'
                 ORDER BY s.path"
            )?;
            let rows = stmt.query_map(params![], |row| {
                Ok(BucketedOobFile {
                    inode: row.get(0)?,
                    path: row.get(1)?,
                    bucket: ConflictBucket::Conflict,
                })
            })?;
            for row in rows { files.push(row?); }
        }

        files.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(files)
    }

    /// Get files with MovedFile signals (same inode, different path).
    ///
    /// Reads from typed signal_moved_file table: inode, path (new), old_path, zones.
    pub fn get_moved_files(&self) -> Result<Vec<crate::corpus::db::types::MovedFileInfo>> {
        use crate::corpus::db::types::MovedFileInfo;

        let mut stmt = self.conn.prepare(
            "SELECT inode, old_path, path, old_zone, new_zone FROM signal_moved_file ORDER BY path"
        )?;

        let files: Vec<MovedFileInfo> = stmt.query_map(params![], |row| {
            Ok(MovedFileInfo {
                inode: row.get(0)?,
                old_path: row.get(1)?,
                new_path: row.get(2)?,
                old_zone: row.get(3)?,
                new_zone: row.get(4)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        Ok(files)
    }
}
