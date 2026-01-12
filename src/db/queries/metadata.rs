//! App metadata, artist canonicalization, and tag mismatch operations.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;

use super::Database;
use crate::db::types::ArtistCanonicalization;

impl Database {
    // ========================================================================
    // Artist Canonicalization Operations
    // ========================================================================

    /// Insert or update an artist canonicalization.
    pub fn upsert_artist_canonicalization(&self, canon: &ArtistCanonicalization) -> Result<i64> {
        self.conn
            .execute(
                r#"INSERT INTO artist_canonicalization
                   (canonical_name, variant_name, confidence, auto_detected, confirmed_at)
                   VALUES (?1, ?2, ?3, ?4, ?5)
                   ON CONFLICT(variant_name) DO UPDATE SET
                       canonical_name = ?1,
                       confidence = ?3,
                       auto_detected = ?4,
                       confirmed_at = ?5"#,
                params![
                    &canon.canonical_name,
                    &canon.variant_name,
                    canon.confidence,
                    canon.auto_detected as i32,
                    &canon.confirmed_at,
                ],
            )
            .context("Failed to upsert artist canonicalization")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get canonical name for a variant.
    pub fn get_canonical_artist(&self, variant_name: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT canonical_name FROM artist_canonicalization WHERE variant_name = ?1",
                params![variant_name],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query canonical artist")
    }

    /// Get all unconfirmed artist canonicalizations.
    pub fn get_unconfirmed_canonicalizations(&self) -> Result<Vec<ArtistCanonicalization>> {
        let mut stmt = self.conn.prepare(
            r#"SELECT id, canonical_name, variant_name, confidence, auto_detected, confirmed_at
               FROM artist_canonicalization
               WHERE confirmed_at IS NULL
               ORDER BY confidence DESC"#,
        )?;

        let rows = stmt.query_map(params![], Self::row_to_artist_canonicalization)?;

        let mut canons = Vec::new();
        for row in rows {
            canons.push(row?);
        }
        Ok(canons)
    }

    /// Confirm an artist canonicalization.
    pub fn confirm_artist_canonicalization(&self, id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE artist_canonicalization SET confirmed_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![id],
            )
            .context("Failed to confirm artist canonicalization")?;
        Ok(())
    }

    /// Get all variants for a canonical name.
    pub fn get_artist_variants(&self, canonical_name: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT variant_name FROM artist_canonicalization WHERE canonical_name = ?1",
        )?;

        let rows = stmt.query_map(params![canonical_name], |row| row.get(0))?;

        let mut variants = Vec::new();
        for row in rows {
            variants.push(row?);
        }
        Ok(variants)
    }

    /// Get artist names grouped by normalized form, with track counts.
    /// Returns only buckets with 2+ unique variants.
    /// Each bucket contains: (normalized_key, [(variant_name, track_count), ...])
    /// Variants are sorted descending by track count within each bucket.
    pub fn get_artist_canonicalization_buckets(
        &self,
    ) -> Result<Vec<(String, Vec<(String, usize)>)>> {
        // First, get all artist names with their counts from corpus tracks
        let mut stmt = self.conn.prepare(
            r#"SELECT artist, COUNT(*) as track_count
               FROM tracks
               WHERE source = 'corpus' AND artist IS NOT NULL AND artist != ''
               GROUP BY artist
               ORDER BY track_count DESC"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            let artist: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((artist, count as usize))
        })?;

        // Collect all artist -> count pairs
        let mut artist_counts: Vec<(String, usize)> = Vec::new();
        for row in rows {
            artist_counts.push(row?);
        }

        // Group by normalized form
        let mut buckets: HashMap<String, Vec<(String, usize)>> = HashMap::new();

        for (artist, count) in artist_counts {
            let normalized = artist.to_lowercase().trim().to_string();
            buckets.entry(normalized).or_default().push((artist, count));
        }

        // Filter to buckets with 2+ unique variants and sort each bucket
        let mut result: Vec<(String, Vec<(String, usize)>)> = buckets
            .into_iter()
            .filter(|(_, variants)| variants.len() >= 2)
            .map(|(key, mut variants)| {
                // Sort by count descending
                variants.sort_by(|a, b| b.1.cmp(&a.1));
                (key, variants)
            })
            .collect();

        // Sort buckets by total track count descending (most impactful first)
        result.sort_by(|a, b| {
            let a_total: usize = a.1.iter().map(|(_, c)| c).sum();
            let b_total: usize = b.1.iter().map(|(_, c)| c).sum();
            b_total.cmp(&a_total)
        });

        Ok(result)
    }

    /// Get track IDs for a specific artist name (exact match, corpus only).
    pub fn get_track_ids_by_artist(&self, artist_name: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM tracks WHERE artist = ?1 AND source = 'corpus'",
        )?;

        let rows = stmt.query_map(params![artist_name], |row| row.get(0))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Bulk update artist name for multiple tracks.
    /// Returns the number of tracks updated.
    pub fn update_artist_for_tracks(&self, track_ids: &[i64], new_artist: &str) -> Result<usize> {
        if track_ids.is_empty() {
            return Ok(0);
        }

        // Build parameterized query for the IN clause
        let placeholders: Vec<String> = (1..=track_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "UPDATE tracks SET artist = ?1 WHERE id IN ({})",
            placeholders.join(", ")
        );

        // Build params vector
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(new_artist.to_string())];
        for id in track_ids {
            params_vec.push(Box::new(*id));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let updated = self
            .conn
            .execute(&sql, params_refs.as_slice())
            .context("Failed to update artist for tracks")?;

        Ok(updated)
    }

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

    // ========================================================================
    // Row Conversion Helper
    // ========================================================================

    pub(super) fn row_to_artist_canonicalization(row: &rusqlite::Row) -> rusqlite::Result<ArtistCanonicalization> {
        let auto_detected_int: i32 = row.get(4)?;

        Ok(ArtistCanonicalization {
            id: Some(row.get(0)?),
            canonical_name: row.get(1)?,
            variant_name: row.get(2)?,
            confidence: row.get(3)?,
            auto_detected: auto_detected_int != 0,
            confirmed_at: row.get(5)?,
        })
    }
}
