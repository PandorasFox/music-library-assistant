//! Tag mismatch operations and OOB tag resolution queries.

use anyhow::Result;
use rusqlite::params;

use super::Database;
use crate::db_thread::SignalWitness;

impl Database {
    // =========================================================================
    // Tag Mismatch Methods (called from db_thread only)
    // =========================================================================

    /// Record a tag mismatch for a track (DB differs from disk).
    /// Uses INSERT OR REPLACE to handle updates.
    ///
    /// Requires witness to prove caller has write authority.
    pub fn record_tag_mismatch(
        &self,
        track_id: i64,
        field: &str,
        db_value: Option<&str>,
        disk_value: Option<&str>,
        _witness: &impl SignalWitness,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO tag_mismatches (track_id, field, db_value, disk_value, created_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
            params![track_id, field, db_value, disk_value],
        )?;
        Ok(())
    }

    /// Clear a specific tag mismatch for a track.
    ///
    /// Requires witness to prove caller has write authority.
    pub fn clear_tag_mismatch(
        &self,
        track_id: i64,
        field: &str,
        _witness: &impl SignalWitness,
    ) -> Result<()> {
        self.conn.execute(
            "DELETE FROM tag_mismatches WHERE track_id = ?1 AND field = ?2",
            params![track_id, field],
        )?;
        Ok(())
    }

    // ========================================================================
    // Tag Collision Detection Queries (for canonicalization signals)
    // ========================================================================

    /// Query distinct tag values with track counts from track_tags table.
    /// Only considers corpus tracks (excludes library tracks and orphaned tag entries).
    /// Returns Vec of (tag_value, track_count).
    pub fn get_distinct_tag_values(&self, tag_name: &str) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT tt.tag_value, COUNT(DISTINCT tt.track_id) as track_count
               FROM track_tags tt
               INNER JOIN tracks t ON tt.track_id = t.id AND t.source = 'corpus'
               WHERE LOWER(tt.tag_name) = LOWER(?1) AND tt.tag_value IS NOT NULL AND tt.tag_value != ''
               GROUP BY tt.tag_value
               ORDER BY track_count DESC"#,
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
    /// Returns track-level data to allow filtering by ISRC/catalog_number.
    /// Only considers corpus tracks (excludes library tracks and orphaned tag entries).
    /// Returns Vec of (album_value, artist_context, isrc, catalog_number).
    ///
    /// TODO: Expand query to include additional release identifiers when we need them:
    /// - MUSICBRAINZ_ALBUMID, MUSICBRAINZ_RELEASEGROUPID
    /// - DISCOGS_RELEASE_ID
    /// - BARCODE
    pub fn get_album_data_for_collision_detection(
        &self,
    ) -> Result<Vec<(String, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   album.tag_value as album,
                   COALESCE(album_artist.tag_value, artist.tag_value, '') as artist_context,
                   COALESCE(isrc.tag_value, '') as isrc,
                   COALESCE(catalog.tag_value, '') as catalog_number
               FROM track_tags album
               INNER JOIN tracks t ON album.track_id = t.id AND t.source = 'corpus'
               LEFT JOIN track_tags album_artist
                   ON album.track_id = album_artist.track_id
                   AND LOWER(album_artist.tag_name) = 'album_artist'
               LEFT JOIN track_tags artist
                   ON album.track_id = artist.track_id
                   AND LOWER(artist.tag_name) = 'artist'
               LEFT JOIN track_tags isrc
                   ON album.track_id = isrc.track_id
                   AND LOWER(isrc.tag_name) = 'isrc'
               LEFT JOIN track_tags catalog
                   ON album.track_id = catalog.track_id
                   AND LOWER(catalog.tag_name) = 'catalognumber'
               WHERE LOWER(album.tag_name) = 'album'
                   AND album.tag_value IS NOT NULL
                   AND album.tag_value != ''"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    // ========================================================================
    // OOB Tag Resolution Queries
    // ========================================================================

    /// Get files with purely one-directional tag mismatches (sync-eligible).
    ///
    /// Joins `signals` (type `oob_tag_sync`) with `tracks` and `tag_mismatches`.
    /// Groups mismatches per track and determines direction:
    /// - All db_value NULL → DiskToIndex
    /// - All disk_value NULL → IndexToDisk
    pub fn get_oob_sync_files(&self) -> Result<Vec<crate::corpus::db::types::OobSyncFile>> {
        use crate::corpus::db::types::{OobSyncDirection, OobSyncFile, TagMismatchEntry};

        // Get all tracks with oob_tag_sync signals
        // signals.issue_key stores the file path for file-level signals
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, s.issue_key
             FROM signals s
             INNER JOIN tracks t ON t.path = s.issue_key AND t.source = 'corpus'
             WHERE s.issue_type = 'oob_tag_sync'"
        )?;

        let track_rows: Vec<(i64, String)> = stmt.query_map(params![], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?))
        })?.filter_map(|r| r.ok()).collect();

        let mut files = Vec::new();

        // For each track, load mismatches and determine direction
        let mut mismatch_stmt = self.conn.prepare(
            "SELECT field, db_value, disk_value FROM tag_mismatches WHERE track_id = ?1 ORDER BY field"
        )?;

        for (track_id, path) in track_rows {
            let mismatches: Vec<TagMismatchEntry> = mismatch_stmt.query_map(params![track_id], |row| {
                Ok(TagMismatchEntry {
                    field: row.get(0)?,
                    db_value: row.get(1)?,
                    disk_value: row.get(2)?,
                    // Individual values not available from tag_mismatches table
                    // (display-only context, mutations use live disk/DB reads)
                    db_values: Vec::new(),
                    disk_values: Vec::new(),
                })
            })?.filter_map(|r| r.ok()).collect();

            if mismatches.is_empty() {
                continue;
            }

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
                track_id,
                path,
                direction,
                mismatches,
            });
        }

        Ok(files)
    }

    /// Get ALL OOB signal files classified into resolution buckets.
    ///
    /// Uses the `tag_mismatches` table for fast SQL-based classification:
    /// - Bucket 0 (MtimeOnly): mtime_only_mismatch signal (mtime changed, tags identical)
    /// - Bucket 1 (DbOnly): all mismatches have disk_value IS NULL
    /// - Bucket 2 (DiskOnly): all mismatches have db_value IS NULL
    /// - Bucket 3 (Conflict): mismatches in both directions or value conflicts
    ///
    /// Loads from all OOB signal types (mtime_only_mismatch, conflict, sync, legacy).
    pub fn get_oob_files_bucketed(&self) -> Result<Vec<crate::corpus::db::types::BucketedOobFile>> {
        use crate::corpus::db::types::{BucketedOobFile, ConflictBucket};

        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, s.issue_key,
                CASE
                    WHEN s.issue_type = 'mtime_only_mismatch' THEN 0
                    WHEN NOT EXISTS(SELECT 1 FROM tag_mismatches tm WHERE tm.track_id = t.id AND tm.disk_value IS NOT NULL) THEN 1
                    WHEN NOT EXISTS(SELECT 1 FROM tag_mismatches tm WHERE tm.track_id = t.id AND tm.db_value IS NOT NULL) THEN 2
                    ELSE 3
                END as bucket
             FROM signals s
             INNER JOIN tracks t ON t.path = s.issue_key AND t.source = 'corpus'
             WHERE s.issue_type IN ('mtime_only_mismatch', 'oob_tag_conflict', 'oob_tag', 'oob_tag_sync')
             ORDER BY bucket, s.issue_key"
        )?;

        let files: Vec<BucketedOobFile> = stmt.query_map(params![], |row| {
            let bucket_int: i32 = row.get(2)?;
            Ok(BucketedOobFile {
                track_id: row.get(0)?,
                path: row.get(1)?,
                bucket: ConflictBucket::from_int(bucket_int),
            })
        })?.filter_map(|r| r.ok()).collect();

        Ok(files)
    }

    /// Get files with InodeChanged signals for the acknowledgement flow.
    ///
    /// Returns files where the inode changed (file was replaced).
    /// Extracts old_inode and new_inode from signal metadata.
    pub fn get_inode_changed_files(&self) -> Result<Vec<crate::corpus::db::types::InodeChangedFile>> {
        use crate::corpus::db::types::InodeChangedFile;

        let mut stmt = self.conn.prepare(
            "SELECT t.id, s.issue_key,
                json_extract(s.metadata_json, '$.old_inode') as old_inode,
                json_extract(s.metadata_json, '$.new_inode') as new_inode
             FROM signals s
             INNER JOIN tracks t ON t.path = s.issue_key AND t.source = 'corpus'
             WHERE s.issue_type = 'inode_changed'
             ORDER BY s.issue_key"
        )?;

        let files: Vec<InodeChangedFile> = stmt.query_map(params![], |row| {
            Ok(InodeChangedFile {
                track_id: row.get(0)?,
                path: row.get(1)?,
                old_inode: row.get(2)?,
                new_inode: row.get(3)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        Ok(files)
    }
}
