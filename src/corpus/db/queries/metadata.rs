//! Tag mismatch operations and OOB tag resolution queries.
//!
//! NOTE: The tag_mismatches table has been removed in the new schema.
//! Tag conflicts are now stored as OOB signals with mismatch details in metadata_json.
//! The record/clear methods are no-ops until callers are fully migrated to signals.

use anyhow::Result;
use rusqlite::params;

use super::Database;
use crate::db_thread::SignalWitness;

impl Database {
    // =========================================================================
    // Tag Mismatch Methods (DEPRECATED - tag_mismatches table removed)
    // =========================================================================

    /// Record a tag mismatch for a file (DB differs from disk).
    ///
    /// **DEPRECATED**: tag_mismatches table is removed. Tag conflicts are now
    /// stored as OOB signals with mismatch details in metadata_json.
    /// This is a no-op until callers are migrated.
    pub fn record_tag_mismatch(
        &self,
        _inode: i64,
        _field: &str,
        _db_value: Option<&str>,
        _disk_value: Option<&str>,
        _witness: &impl SignalWitness,
    ) -> Result<()> {
        // TODO: Convert to signal-based mismatch tracking
        Ok(())
    }

    /// Clear a specific tag mismatch for a file.
    ///
    /// **DEPRECATED**: tag_mismatches table is removed.
    /// This is a no-op until callers are migrated.
    pub fn clear_tag_mismatch(
        &self,
        _inode: i64,
        _field: &str,
        _witness: &impl SignalWitness,
    ) -> Result<()> {
        // TODO: Convert to signal-based mismatch tracking
        Ok(())
    }

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
               INNER JOIN files f ON ct.inode = f.inode AND f.source = 'corpus'
               WHERE LOWER(ct.tag_name) = LOWER(?1) AND ct.tag_value IS NOT NULL AND ct.tag_value != ''
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
    ) -> Result<Vec<(String, String, String, String)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   album.tag_value as album,
                   COALESCE(album_artist.tag_value, artist.tag_value, '') as artist_context,
                   COALESCE(isrc.tag_value, '') as isrc,
                   COALESCE(catalog.tag_value, '') as catalog_number
               FROM corpus_tags album
               INNER JOIN files f ON album.inode = f.inode AND f.source = 'corpus'
               LEFT JOIN corpus_tags album_artist
                   ON album.inode = album_artist.inode
                   AND LOWER(album_artist.tag_name) = 'album_artist'
               LEFT JOIN corpus_tags artist
                   ON album.inode = artist.inode
                   AND LOWER(artist.tag_name) = 'artist'
               LEFT JOIN corpus_tags isrc
                   ON album.inode = isrc.inode
                   AND LOWER(isrc.tag_name) = 'isrc'
               LEFT JOIN corpus_tags catalog
                   ON album.inode = catalog.inode
                   AND LOWER(catalog.tag_name) = 'catalog_number'
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
    /// NOTE: tag_mismatches table is removed. This now extracts mismatch info
    /// from signal metadata_json (field 'mismatches' array).
    pub fn get_oob_sync_files(&self) -> Result<Vec<crate::corpus::db::types::OobSyncFile>> {
        use crate::corpus::db::types::{OobSyncDirection, OobSyncFile, TagMismatchEntry};

        // Get all files with oob_tag_sync signals
        // signals.issue_key stores the file path for file-level signals
        let mut stmt = self.conn.prepare(
            "SELECT f.inode, s.issue_key, s.metadata_json
             FROM signals s
             INNER JOIN files f ON f.path = s.issue_key AND f.source = 'corpus'
             WHERE s.issue_type = 'oob_tag_sync'"
        )?;

        let mut files = Vec::new();

        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?;

        for row in rows {
            let (inode, path, metadata_json) = row?;

            // Parse mismatches from metadata_json
            let mismatches: Vec<TagMismatchEntry> = if let Some(json) = metadata_json {
                serde_json::from_str::<serde_json::Value>(&json)
                    .ok()
                    .and_then(|v| v.get("mismatches").cloned())
                    .and_then(|arr| {
                        arr.as_array().map(|items| {
                            items.iter().filter_map(|item| {
                                Some(TagMismatchEntry {
                                    field: item.get("field")?.as_str()?.to_string(),
                                    db_value: item.get("db_value").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    disk_value: item.get("disk_value").and_then(|v| v.as_str()).map(|s| s.to_string()),
                                    _db_values: Vec::new(),
                                    _disk_values: Vec::new(),
                                })
                            }).collect()
                        })
                    })
                    .unwrap_or_default()
            } else {
                Vec::new()
            };

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
                track_id: inode, // Using inode as track_id for compatibility
                path,
                direction,
                mismatches,
            });
        }

        Ok(files)
    }

    /// Get ALL OOB signal files classified into resolution buckets.
    ///
    /// NOTE: tag_mismatches table is removed. Classification now comes from
    /// signal metadata_json (field 'bucket' or parsed from 'mismatches').
    /// - Bucket 0 (MtimeOnly): mtime_only_mismatch signal
    /// - Bucket 1 (DbOnly): all mismatches have disk_value NULL
    /// - Bucket 2 (DiskOnly): all mismatches have db_value NULL
    /// - Bucket 3 (Conflict): mismatches in both directions
    pub fn get_oob_files_bucketed(&self) -> Result<Vec<crate::corpus::db::types::BucketedOobFile>> {
        use crate::corpus::db::types::{BucketedOobFile, ConflictBucket};

        let mut stmt = self.conn.prepare(
            "SELECT f.inode, s.issue_key, s.issue_type, s.metadata_json
             FROM signals s
             INNER JOIN files f ON f.path = s.issue_key AND f.source = 'corpus'
             WHERE s.issue_type IN ('mtime_only_mismatch', 'oob_tag_conflict', 'oob_tag', 'oob_tag_sync')
             ORDER BY s.issue_key"
        )?;

        let mut files = Vec::new();
        let rows = stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Option<String>>(3)?,
            ))
        })?;

        for row in rows {
            let (inode, path, issue_type, metadata_json) = row?;

            let bucket = if issue_type == "mtime_only_mismatch" {
                ConflictBucket::MtimeOnly
            } else if let Some(json) = &metadata_json {
                // Try to parse bucket from metadata, or infer from mismatches
                let parsed: Option<ConflictBucket> = serde_json::from_str::<serde_json::Value>(json)
                    .ok()
                    .and_then(|v| {
                        // Check for explicit bucket field
                        if let Some(b) = v.get("bucket").and_then(|b| b.as_i64()) {
                            return Some(ConflictBucket::from_int(b as i32));
                        }
                        // Infer from mismatches
                        v.get("mismatches").and_then(|arr| arr.as_array()).map(|items| {
                            let all_db_null = items.iter().all(|i| i.get("db_value").map_or(true, |v| v.is_null()));
                            let all_disk_null = items.iter().all(|i| i.get("disk_value").map_or(true, |v| v.is_null()));
                            if all_db_null { ConflictBucket::DiskOnly }
                            else if all_disk_null { ConflictBucket::DbOnly }
                            else { ConflictBucket::Conflict }
                        })
                    });
                parsed.unwrap_or(ConflictBucket::Conflict)
            } else {
                ConflictBucket::Conflict
            };

            files.push(BucketedOobFile {
                track_id: inode, // Using inode as track_id for compatibility
                path,
                bucket,
            });
        }

        Ok(files)
    }

    /// Get files with InodeChanged signals for the acknowledgement flow.
    ///
    /// Returns files where the inode changed (file was replaced).
    /// Extracts old_inode and new_inode from signal metadata.
    pub fn get_inode_changed_files(&self) -> Result<Vec<crate::corpus::db::types::InodeChangedFile>> {
        use crate::corpus::db::types::InodeChangedFile;

        let mut stmt = self.conn.prepare(
            "SELECT f.inode, s.issue_key,
                json_extract(s.metadata_json, '$.old_inode') as old_inode,
                json_extract(s.metadata_json, '$.new_inode') as new_inode
             FROM signals s
             INNER JOIN files f ON f.path = s.issue_key AND f.source = 'corpus'
             WHERE s.issue_type = 'inode_changed'
             ORDER BY s.issue_key"
        )?;

        let files: Vec<InodeChangedFile> = stmt.query_map(params![], |row| {
            Ok(InodeChangedFile {
                track_id: row.get(0)?, // Using inode as track_id
                path: row.get(1)?,
                old_inode: row.get(2)?,
                new_inode: row.get(3)?,
            })
        })?.filter_map(|r| r.ok()).collect();

        Ok(files)
    }
}
