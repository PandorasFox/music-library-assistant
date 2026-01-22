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
    /// Returns Vec of (tag_value, track_count).
    pub fn get_distinct_tag_values(&self, tag_name: &str) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT tag_value, COUNT(DISTINCT track_id) as track_count
               FROM track_tags
               WHERE LOWER(tag_name) = LOWER(?1) AND tag_value IS NOT NULL AND tag_value != ''
               GROUP BY tag_value
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
    /// Returns Vec of (album_value, artist_context, track_count).
    pub fn get_album_values_with_artist_context(&self) -> Result<Vec<(String, String, usize)>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT
                   album.tag_value as album,
                   COALESCE(album_artist.tag_value, artist.tag_value, '') as artist_context,
                   COUNT(DISTINCT album.track_id) as track_count
               FROM track_tags album
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
}
