//! App metadata and tag mismatch operations.

use anyhow::Result;
use rusqlite::params;

use super::Database;

impl Database {
    // ========================================================================
    // App Metadata
    // ========================================================================

    /// Get a metadata value by key.
    pub fn get_metadata(&self, key: &str) -> Result<Option<String>> {
        let result = self.conn.query_row(
            "SELECT value FROM app_metadata WHERE key = ?1",
            params![key],
            |row| row.get(0),
        );

        match result {
            Ok(value) => Ok(Some(value)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Set a metadata value (upsert).
    pub fn set_metadata(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO app_metadata (key, value, updated_at)
             VALUES (?1, ?2, CURRENT_TIMESTAMP)
             ON CONFLICT(key) DO UPDATE SET
                value = excluded.value,
                updated_at = CURRENT_TIMESTAMP",
            params![key, value],
        )?;
        Ok(())
    }

    /// Get health data version from metadata.
    pub fn get_health_version(&self) -> Result<Option<u32>> {
        match self.get_metadata("health_version")? {
            Some(v) => Ok(v.parse().ok()),
            None => Ok(None),
        }
    }

    /// Set health data version.
    pub fn set_health_version(&self, version: u32) -> Result<()> {
        self.set_metadata("health_version", &version.to_string())
    }

    // =========================================================================
    // Tag Mismatch Methods
    // =========================================================================

    /// Record a tag mismatch for a track (DB differs from disk)
    /// Uses INSERT OR REPLACE to handle updates
    pub fn record_tag_mismatch(
        &self,
        track_id: i64,
        field: &str,
        db_value: Option<&str>,
        disk_value: Option<&str>,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO tag_mismatches (track_id, field, db_value, disk_value, created_at)
             VALUES (?1, ?2, ?3, ?4, CURRENT_TIMESTAMP)",
            params![track_id, field, db_value, disk_value],
        )?;
        Ok(())
    }

    /// Clear a specific tag mismatch for a track
    pub fn clear_tag_mismatch(&self, track_id: i64, field: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM tag_mismatches WHERE track_id = ?1 AND field = ?2",
            params![track_id, field],
        )?;
        Ok(())
    }

    /// Clear all tag mismatches for a track
    pub fn clear_tag_mismatches_for_track(&self, track_id: i64) -> Result<()> {
        self.conn.execute(
            "DELETE FROM tag_mismatches WHERE track_id = ?1",
            params![track_id],
        )?;
        Ok(())
    }

    /// Clear tag mismatches by track path
    pub fn clear_tag_mismatches_by_path(&self, path: &str) -> Result<()> {
        self.conn.execute(
            "DELETE FROM tag_mismatches WHERE track_id IN (SELECT id FROM tracks WHERE path = ?1)",
            params![path],
        )?;
        Ok(())
    }

    /// Get count of tracks with tag mismatches
    pub fn get_tag_mismatch_count(&self) -> Result<usize> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT track_id) FROM tag_mismatches",
            params![],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }

    /// Check if a track has any tag mismatches
    pub fn has_tag_mismatch(&self, track_id: i64) -> Result<bool> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM tag_mismatches WHERE track_id = ?1",
            params![track_id],
            |row| row.get(0),
        )?;
        Ok(count > 0)
    }

    /// Get all tag mismatches grouped by track
    /// Returns Vec of (track_id, field, db_value, disk_value)
    pub fn get_all_tag_mismatches(&self) -> Result<Vec<(i64, String, Option<String>, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT track_id, field, db_value, disk_value FROM tag_mismatches ORDER BY track_id, field"
        )?;
        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Get tag mismatches for a specific track
    /// Returns Vec of (field, db_value, disk_value)
    pub fn get_tag_mismatches_for_track(&self, track_id: i64) -> Result<Vec<(String, Option<String>, Option<String>)>> {
        let mut stmt = self.conn.prepare(
            "SELECT field, db_value, disk_value FROM tag_mismatches WHERE track_id = ?1 ORDER BY field"
        )?;
        let rows = stmt.query_map(params![track_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
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

    /// Query album values with artist context for collision detection.
    /// Albums are keyed by (artist_context, album) to avoid false positives
    /// like "Greatest Hits" by different artists.
    /// Only considers corpus tracks (excludes library tracks and orphaned tag entries).
    /// Returns Vec of (album_value, artist_context, track_count).
    pub fn get_album_values_with_artist_context(&self) -> Result<Vec<(String, String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   album.tag_value as album,
                   COALESCE(album_artist.tag_value, artist.tag_value, '') as artist_context,
                   COUNT(DISTINCT album.track_id) as track_count
               FROM track_tags album
               INNER JOIN tracks t ON album.track_id = t.id AND t.source = 'corpus'
               LEFT JOIN track_tags album_artist
                   ON album.track_id = album_artist.track_id
                   AND LOWER(album_artist.tag_name) = 'album_artist'
               LEFT JOIN track_tags artist
                   ON album.track_id = artist.track_id
                   AND LOWER(artist.tag_name) = 'artist'
               WHERE LOWER(album.tag_name) = 'album'
                   AND album.tag_value IS NOT NULL
                   AND album.tag_value != ''
               GROUP BY album.tag_value, artist_context
               ORDER BY track_count DESC"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            let album: String = row.get(0)?;
            let artist_context: String = row.get(1)?;
            let count: i64 = row.get(2)?;
            Ok((album, artist_context, count as usize))
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

    /// Get files with OOB tag conflict signals.
    ///
    /// Returns a flat file list from signals (track_id + path). Mismatch detail
    /// is computed on-demand per file, because the tag_mismatches table requires
    /// write access that computations cannot provide on read-only connections.
    pub fn get_oob_conflict_files(&self) -> Result<Vec<crate::corpus::db::types::OobSignalFile>> {
        use crate::corpus::db::types::OobSignalFile;

        // Get all tracks with oob_tag_conflict signals (include legacy "oob_tag")
        // signals.issue_key stores the file path for file-level signals
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT t.id, s.issue_key
             FROM signals s
             INNER JOIN tracks t ON t.path = s.issue_key AND t.source = 'corpus'
             WHERE s.issue_type IN ('oob_tag_conflict', 'oob_tag')
             ORDER BY s.issue_key"
        )?;

        let files: Vec<OobSignalFile> = stmt.query_map(params![], |row| {
            Ok(OobSignalFile {
                track_id: row.get(0)?,
                path: row.get(1)?,
            })
        })?.filter_map(|r| r.ok()).collect();

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
