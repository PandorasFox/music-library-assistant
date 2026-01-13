//! App metadata, tag canonicalization, and tag mismatch operations.

use anyhow::{Context, Result};
use rusqlite::{params, OptionalExtension};
use std::collections::HashMap;

use super::Database;
use crate::corpus::db::types::{AlbumArtistCollation, AlbumArtistPopulationGroup, TagCanonicalization};

impl Database {
    // ========================================================================
    // Unified Tag Canonicalization Operations
    // ========================================================================

    /// Insert or update a tag canonicalization (unified for artist, album_artist, genre, album).
    pub fn upsert_tag_canonicalization(&self, canon: &TagCanonicalization) -> Result<i64> {
        self.conn
            .execute(
                r#"INSERT INTO tag_canonicalization
                   (tag_name, canonical_value, variant_value, confidence, auto_detected, confirmed_at)
                   VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                   ON CONFLICT(tag_name, variant_value) DO UPDATE SET
                       canonical_value = ?2,
                       confidence = ?4,
                       auto_detected = ?5,
                       confirmed_at = ?6"#,
                params![
                    &canon.tag_name,
                    &canon.canonical_value,
                    &canon.variant_value,
                    canon.confidence,
                    canon.auto_detected as i32,
                    &canon.confirmed_at,
                ],
            )
            .context("Failed to upsert tag canonicalization")?;

        Ok(self.conn.last_insert_rowid())
    }

    /// Get canonical value for a variant of a specific tag.
    pub fn get_canonical_tag_value(&self, tag_name: &str, variant_value: &str) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT canonical_value FROM tag_canonicalization WHERE tag_name = ?1 AND variant_value = ?2",
                params![tag_name, variant_value],
                |row| row.get(0),
            )
            .optional()
            .context("Failed to query canonical tag value")
    }

    /// Get all unconfirmed tag canonicalizations, optionally filtered by tag_name.
    pub fn get_unconfirmed_tag_canonicalizations(&self, tag_name: Option<&str>) -> Result<Vec<TagCanonicalization>> {
        let mut canons = Vec::new();

        if let Some(name) = tag_name {
            let mut stmt = self.conn.prepare(
                r#"SELECT id, tag_name, canonical_value, variant_value, confidence, auto_detected, confirmed_at
                   FROM tag_canonicalization
                   WHERE confirmed_at IS NULL AND tag_name = ?1
                   ORDER BY confidence DESC"#,
            )?;
            let rows = stmt.query_map(params![name], Self::row_to_tag_canonicalization)?;
            for row in rows {
                canons.push(row?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                r#"SELECT id, tag_name, canonical_value, variant_value, confidence, auto_detected, confirmed_at
                   FROM tag_canonicalization
                   WHERE confirmed_at IS NULL
                   ORDER BY tag_name, confidence DESC"#,
            )?;
            let rows = stmt.query_map(params![], Self::row_to_tag_canonicalization)?;
            for row in rows {
                canons.push(row?);
            }
        }

        Ok(canons)
    }

    /// Confirm a tag canonicalization.
    pub fn confirm_tag_canonicalization(&self, id: i64) -> Result<()> {
        self.conn
            .execute(
                "UPDATE tag_canonicalization SET confirmed_at = CURRENT_TIMESTAMP WHERE id = ?1",
                params![id],
            )
            .context("Failed to confirm tag canonicalization")?;
        Ok(())
    }

    /// Get all variant values for a canonical value of a specific tag.
    pub fn get_tag_variants(&self, tag_name: &str, canonical_value: &str) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT variant_value FROM tag_canonicalization WHERE tag_name = ?1 AND canonical_value = ?2",
        )?;

        let rows = stmt.query_map(params![tag_name, canonical_value], |row| row.get(0))?;

        let mut variants = Vec::new();
        for row in rows {
            variants.push(row?);
        }
        Ok(variants)
    }

    /// Count canonicalization issues by tag name.
    pub fn count_tag_canonicalizations(&self, tag_name: Option<&str>) -> Result<usize> {
        let count: i64 = if let Some(name) = tag_name {
            self.conn.query_row(
                "SELECT COUNT(*) FROM tag_canonicalization WHERE tag_name = ?1",
                params![name],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM tag_canonicalization",
                params![],
                |row| row.get(0),
            )?
        };
        Ok(count as usize)
    }

    /// Count unconfirmed canonicalizations by tag name.
    pub fn count_unconfirmed_tag_canonicalizations(&self, tag_name: Option<&str>) -> Result<usize> {
        let count: i64 = if let Some(name) = tag_name {
            self.conn.query_row(
                "SELECT COUNT(*) FROM tag_canonicalization WHERE tag_name = ?1 AND confirmed_at IS NULL",
                params![name],
                |row| row.get(0),
            )?
        } else {
            self.conn.query_row(
                "SELECT COUNT(*) FROM tag_canonicalization WHERE confirmed_at IS NULL",
                params![],
                |row| row.get(0),
            )?
        };
        Ok(count as usize)
    }

    // ========================================================================
    // Backwards-Compatible Artist Canonicalization Wrappers
    // ========================================================================

    /// Get canonical artist name for a variant (convenience wrapper).
    pub fn get_canonical_artist(&self, variant_name: &str) -> Result<Option<String>> {
        self.get_canonical_tag_value("artist", variant_name)
    }

    /// Get all variants for a canonical artist name (convenience wrapper).
    pub fn get_artist_variants(&self, canonical_name: &str) -> Result<Vec<String>> {
        self.get_tag_variants("artist", canonical_name)
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
    // Album Artist Canonicalization
    // ========================================================================

    /// Get album_artist names grouped by normalized form, with track counts.
    /// Returns only buckets with 2+ unique variants.
    /// Each bucket contains: (normalized_key, [(variant_name, track_count), ...])
    /// Variants are sorted descending by track count within each bucket.
    pub fn get_album_artist_canonicalization_buckets(
        &self,
    ) -> Result<Vec<(String, Vec<(String, usize)>)>> {
        // Get all album_artist names with their counts from corpus tracks
        let mut stmt = self.conn.prepare(
            r#"SELECT album_artist, COUNT(*) as track_count
               FROM tracks
               WHERE source = 'corpus' AND album_artist IS NOT NULL AND album_artist != ''
               GROUP BY album_artist
               ORDER BY track_count DESC"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            let album_artist: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((album_artist, count as usize))
        })?;

        // Collect all album_artist -> count pairs
        let mut artist_counts: Vec<(String, usize)> = Vec::new();
        for row in rows {
            artist_counts.push(row?);
        }

        // Group by normalized form
        let mut buckets: HashMap<String, Vec<(String, usize)>> = HashMap::new();

        for (album_artist, count) in artist_counts {
            let normalized = album_artist.to_lowercase().trim().to_string();
            buckets.entry(normalized).or_default().push((album_artist, count));
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

    /// Get track IDs for a specific album_artist name (exact match, corpus only).
    pub fn get_track_ids_by_album_artist(&self, album_artist_name: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM tracks WHERE album_artist = ?1 AND source = 'corpus'",
        )?;

        let rows = stmt.query_map(params![album_artist_name], |row| row.get(0))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Bulk update album_artist name for multiple tracks.
    /// Returns the number of tracks updated.
    pub fn update_album_artist_for_tracks(&self, track_ids: &[i64], new_album_artist: &str) -> Result<usize> {
        if track_ids.is_empty() {
            return Ok(0);
        }

        // Build parameterized query for the IN clause
        let placeholders: Vec<String> = (1..=track_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "UPDATE tracks SET album_artist = ?1 WHERE id IN ({})",
            placeholders.join(", ")
        );

        // Build params vector
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(new_album_artist.to_string())];
        for id in track_ids {
            params_vec.push(Box::new(*id));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let updated = self
            .conn
            .execute(&sql, params_refs.as_slice())
            .context("Failed to update album_artist for tracks")?;

        Ok(updated)
    }

    /// Get full tracks for a specific album_artist name (for quality analysis).
    pub fn get_tracks_by_album_artist(&self, album_artist_name: &str) -> Result<Vec<crate::corpus::db::types::Track>> {
        // TODO: This query pattern (17-column SELECT for row_to_track) is duplicated across
        // multiple files. Consider extracting a constant or helper for the column list.
        let mut stmt = self.conn.prepare(
            r#"SELECT id, path, source, inode, file_size, file_type,
                      artist, album, album_artist, title, track_number, genre,
                      duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
               FROM tracks
               WHERE album_artist = ?1 AND source = 'corpus'"#,
        )?;

        let rows = stmt.query_map(params![album_artist_name], Self::row_to_track)?;

        let mut tracks = Vec::new();
        for row in rows {
            tracks.push(row?);
        }
        Ok(tracks)
    }

    /// Get full tracks for multiple album_artist names (for bulk quality analysis).
    pub fn get_tracks_by_album_artists(&self, album_artist_names: &[String]) -> Result<Vec<crate::corpus::db::types::Track>> {
        if album_artist_names.is_empty() {
            return Ok(Vec::new());
        }

        // TODO: This query pattern (17-column SELECT for row_to_track) is duplicated across
        // multiple files. Consider extracting a constant or helper for the column list.
        let placeholders: Vec<String> = (1..=album_artist_names.len())
            .map(|i| format!("?{}", i))
            .collect();
        let sql = format!(
            r#"SELECT id, path, source, inode, file_size, file_type,
                      artist, album, album_artist, title, track_number, genre,
                      duration_ms, bitrate_kbps, sample_rate, fingerprint, isrc
               FROM tracks
               WHERE album_artist IN ({}) AND source = 'corpus'"#,
            placeholders.join(", ")
        );

        let mut stmt = self.conn.prepare(&sql)?;

        let params: Vec<&dyn rusqlite::ToSql> = album_artist_names
            .iter()
            .map(|s| s as &dyn rusqlite::ToSql)
            .collect();

        let rows = stmt.query_map(params.as_slice(), Self::row_to_track)?;

        let mut tracks = Vec::new();
        for row in rows {
            tracks.push(row?);
        }
        Ok(tracks)
    }

    // ========================================================================
    // Album Artist Collation (Mixed-Artist Albums)
    // ========================================================================

    /// Find albums with multiple distinct artist values.
    /// Returns albums where tracks have different artists, indicating potential
    /// compilation or "Various Artists" album.
    /// Each result: (album_name, vec of (artist_name, track_count))
    pub fn get_albums_with_multiple_artists(&self) -> Result<Vec<AlbumArtistCollation>> {
        // First, find albums with 2+ distinct artist values
        let mut stmt = self.conn.prepare(
            r#"SELECT album, COUNT(DISTINCT artist) as artist_count
               FROM tracks
               WHERE source = 'corpus'
                 AND album IS NOT NULL
                 AND album != ''
               GROUP BY album
               HAVING artist_count >= 2
               ORDER BY artist_count DESC"#,
        )?;

        let album_rows = stmt.query_map(params![], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i32>(1)?))
        })?;

        let mut results = Vec::new();

        for album_row in album_rows {
            let (album_name, _) = album_row?;

            // Get artists and track counts for this album
            let mut artist_stmt = self.conn.prepare(
                r#"SELECT artist, COUNT(*) as count, album_artist
                   FROM tracks
                   WHERE source = 'corpus' AND album = ?1
                   GROUP BY artist
                   ORDER BY count DESC"#,
            )?;

            let artist_rows = artist_stmt.query_map(params![&album_name], |row| {
                Ok((
                    row.get::<_, Option<String>>(0)?,
                    row.get::<_, usize>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?;

            let mut artists = Vec::new();
            let mut existing_album_artist: Option<String> = None;
            let mut total_tracks = 0usize;

            for artist_row in artist_rows {
                let (artist, count, aa) = artist_row?;
                if let Some(name) = artist {
                    artists.push((name, count));
                    total_tracks += count;
                }
                if existing_album_artist.is_none() && aa.is_some() {
                    existing_album_artist = aa;
                }
            }

            // Suggest album_artist based on artist distribution
            let suggested_album_artist = if artists.len() == 1 {
                // Single artist, use their name
                artists.first().map(|(name, _)| name.clone())
            } else {
                // Multiple artists - check if one dominates (>70% of tracks)
                let dominant = artists.first()
                    .filter(|(_, count)| (*count as f64 / total_tracks as f64) > 0.7)
                    .map(|(name, _)| name.clone());

                dominant.or_else(|| Some("Various Artists".to_string()))
            };

            results.push(AlbumArtistCollation {
                album_name,
                artists,
                total_tracks,
                existing_album_artist,
                suggested_album_artist,
            });
        }

        Ok(results)
    }

    // ========================================================================
    // Album Artist Population (Missing Tags)
    // ========================================================================

    /// Find tracks with missing album_artist tags, grouped by album.
    /// For bulk population of the album_artist field.
    pub fn get_tracks_missing_album_artist(&self) -> Result<Vec<AlbumArtistPopulationGroup>> {
        // Group by album first (tracks with album tags)
        let mut album_stmt = self.conn.prepare(
            r#"SELECT album, artist, COUNT(*) as count
               FROM tracks
               WHERE source = 'corpus'
                 AND (album_artist IS NULL OR album_artist = '')
                 AND album IS NOT NULL AND album != ''
               GROUP BY album, artist
               ORDER BY album, count DESC"#,
        )?;

        let mut album_groups: HashMap<String, Vec<(String, usize)>> = HashMap::new();

        let album_rows = album_stmt.query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, usize>(2)?,
            ))
        })?;

        for row in album_rows {
            let (album, artist, count) = row?;
            let artist_name = artist.unwrap_or_else(|| "[Unknown Artist]".to_string());
            album_groups.entry(album).or_default().push((artist_name, count));
        }

        let mut results: Vec<AlbumArtistPopulationGroup> = album_groups
            .into_iter()
            .map(|(album_name, artists)| {
                let total_tracks: usize = artists.iter().map(|(_, c)| c).sum();
                let suggested = if artists.len() == 1 {
                    artists.first().map(|(name, _)| name.clone())
                } else {
                    Some("Various Artists".to_string())
                };

                AlbumArtistPopulationGroup {
                    album_name: Some(album_name),
                    directory: None,
                    artists,
                    total_tracks,
                    suggested_album_artist: suggested,
                }
            })
            .collect();

        // Also include tracks without album tags, grouped by directory
        let mut dir_stmt = self.conn.prepare(
            r#"SELECT path, artist
               FROM tracks
               WHERE source = 'corpus'
                 AND (album_artist IS NULL OR album_artist = '')
                 AND (album IS NULL OR album = '')"#,
        )?;

        let dir_rows = dir_stmt.query_map(params![], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
        })?;

        // Note: The dir_rows iteration was inefficient. Instead, we use the SQL-based grouping below.
        // Just consume the iterator to avoid warnings.
        for _ in dir_rows {}

        // Re-count properly for directory groups
        let mut dir_stmt2 = self.conn.prepare(
            r#"SELECT
                   substr(path, 1, length(path) - length(replace(path, '/', '')) - length(substr(path, instr(path, '/') + 1))) as dir,
                   artist,
                   COUNT(*) as count
               FROM tracks
               WHERE source = 'corpus'
                 AND (album_artist IS NULL OR album_artist = '')
                 AND (album IS NULL OR album = '')
               GROUP BY dir, artist
               ORDER BY dir, count DESC"#,
        )?;

        let dir_rows2 = dir_stmt2.query_map(params![], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, usize>(2)?,
            ))
        });

        if let Ok(rows) = dir_rows2 {
            let mut dir_groups2: HashMap<String, Vec<(String, usize)>> = HashMap::new();
            for row in rows.flatten() {
                let (dir, artist, count) = row;
                let artist_name = artist.unwrap_or_else(|| "[Unknown Artist]".to_string());
                dir_groups2.entry(dir).or_default().push((artist_name, count));
            }

            for (dir, artists) in dir_groups2 {
                let total_tracks: usize = artists.iter().map(|(_, c)| c).sum();
                let suggested = if artists.len() == 1 {
                    artists.first().map(|(name, _)| name.clone())
                } else {
                    Some("Various Artists".to_string())
                };

                results.push(AlbumArtistPopulationGroup {
                    album_name: None,
                    directory: Some(dir),
                    artists,
                    total_tracks,
                    suggested_album_artist: suggested,
                });
            }
        }

        // Sort: album-based first, then directory-based
        results.sort_by_key(|g| (g.album_name.is_none(), g.total_tracks));
        results.reverse();

        Ok(results)
    }

    // ========================================================================
    // Album Canonicalization
    // ========================================================================

    /// Get album names grouped by normalized form, with track counts.
    /// Uses album normalization to detect EP/LP and edition variants.
    /// Returns only buckets with 2+ unique variants.
    pub fn get_album_canonicalization_buckets(
        &self,
    ) -> Result<Vec<(String, Vec<(String, usize)>)>> {
        use crate::corpus::health::album_normalization::normalize_album;

        // Get all album names with their counts from corpus tracks
        let mut stmt = self.conn.prepare(
            r#"SELECT album, COUNT(*) as track_count
               FROM tracks
               WHERE source = 'corpus' AND album IS NOT NULL AND album != ''
               GROUP BY album
               ORDER BY track_count DESC"#,
        )?;

        let rows = stmt.query_map(params![], |row| {
            let album: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((album, count as usize))
        })?;

        // Collect all album -> count pairs
        let mut album_counts: Vec<(String, usize)> = Vec::new();
        for row in rows {
            album_counts.push(row?);
        }

        // Group by normalized base name
        let mut buckets: HashMap<String, Vec<(String, usize)>> = HashMap::new();

        for (album, count) in album_counts {
            let normalized = normalize_album(&album);
            let key = normalized.base_name.to_lowercase();
            buckets.entry(key).or_default().push((album, count));
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

    /// Get detailed info for a specific album name: artists, directories, and file types.
    /// Used to enhance album variant display with richer context.
    pub fn get_album_variant_details(
        &self,
        album_name: &str,
    ) -> Result<(Vec<String>, Vec<String>, Vec<String>)> {
        // Get distinct artists
        let mut artist_stmt = self.conn.prepare(
            r#"SELECT DISTINCT artist FROM tracks
               WHERE source = 'corpus' AND album = ?1 AND artist IS NOT NULL AND artist != ''
               ORDER BY artist"#,
        )?;
        let artists: Vec<String> = artist_stmt
            .query_map(params![album_name], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        // Get distinct parent directories (extract from path)
        let mut dir_stmt = self.conn.prepare(
            r#"SELECT DISTINCT
                  CASE
                    WHEN INSTR(path, '/') > 0
                    THEN SUBSTR(path, 1, LENGTH(path) - LENGTH(REPLACE(path, '/', '')) - LENGTH(SUBSTR(path, LENGTH(path) - LENGTH(REPLACE(path, '/', '')) + 1)))
                    ELSE path
                  END as dir
               FROM tracks
               WHERE source = 'corpus' AND album = ?1
               ORDER BY dir"#,
        )?;
        let directories: Vec<String> = dir_stmt
            .query_map(params![album_name], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .take(5) // Limit to 5 directories for display
            .collect();

        // Get distinct file extensions
        let mut ext_stmt = self.conn.prepare(
            r#"SELECT DISTINCT
                  LOWER(SUBSTR(path, LENGTH(path) - INSTR(REVERSE(path), '.') + 2)) as ext
               FROM tracks
               WHERE source = 'corpus' AND album = ?1
               ORDER BY ext"#,
        )?;
        let file_types: Vec<String> = ext_stmt
            .query_map(params![album_name], |row| row.get(0))?
            .filter_map(|r| r.ok())
            .collect();

        Ok((artists, directories, file_types))
    }

    /// Get track IDs for a specific album name (exact match, corpus only).
    pub fn get_track_ids_by_album(&self, album_name: &str) -> Result<Vec<i64>> {
        let mut stmt = self.conn.prepare(
            "SELECT id FROM tracks WHERE album = ?1 AND source = 'corpus'",
        )?;

        let rows = stmt.query_map(params![album_name], |row| row.get(0))?;

        let mut ids = Vec::new();
        for row in rows {
            ids.push(row?);
        }
        Ok(ids)
    }

    /// Bulk update album name for multiple tracks.
    /// Returns the number of tracks updated.
    pub fn update_album_for_tracks(&self, track_ids: &[i64], new_album: &str) -> Result<usize> {
        if track_ids.is_empty() {
            return Ok(0);
        }

        // Build parameterized query for the IN clause
        let placeholders: Vec<String> = (1..=track_ids.len())
            .map(|i| format!("?{}", i + 1))
            .collect();
        let sql = format!(
            "UPDATE tracks SET album = ?1 WHERE id IN ({})",
            placeholders.join(", ")
        );

        // Build params vector
        let mut params_vec: Vec<Box<dyn rusqlite::ToSql>> = vec![Box::new(new_album.to_string())];
        for id in track_ids {
            params_vec.push(Box::new(*id));
        }

        let params_refs: Vec<&dyn rusqlite::ToSql> =
            params_vec.iter().map(|p| p.as_ref()).collect();

        let updated = self
            .conn
            .execute(&sql, params_refs.as_slice())
            .context("Failed to update album for tracks")?;

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
    // Row Conversion Helper
    // ========================================================================

    pub(super) fn row_to_tag_canonicalization(row: &rusqlite::Row) -> rusqlite::Result<TagCanonicalization> {
        let auto_detected_int: i32 = row.get(5)?;

        Ok(TagCanonicalization {
            id: Some(row.get(0)?),
            tag_name: row.get(1)?,
            canonical_value: row.get(2)?,
            variant_value: row.get(3)?,
            confidence: row.get(4)?,
            auto_detected: auto_detected_int != 0,
            confirmed_at: row.get(6)?,
        })
    }
}
