//! Album Artist Detection
//!
//! Detects albums that need album_artist resolution:
//! - Same album tag (normalized)
//! - Different artist tags (multiple distinct artists)
//! - Missing OR inconsistent album_artist tags
//!
//! These represent compilations or multi-artist albums where the album_artist
//! field should be set to something like "Various Artists" or a specific artist.

use std::collections::HashMap;

use anyhow::Result;

use crate::corpus::db::Database;
use super::normalization::normalize_artist;

/// An album with inconsistent album_artist that needs resolution.
#[derive(Debug, Clone)]
pub struct AlbumArtistIssue {
    /// The album name (original, most common value)
    pub album: String,
    /// Normalized album key for grouping
    pub normalized_album: String,
    /// Track IDs affected
    pub track_ids: Vec<i64>,
    /// Artist variant counts: "Artist A" → 5 tracks, "Artist B" → 3 tracks
    pub artist_variants: HashMap<String, usize>,
    /// Album artist variant counts: "" → 6 (missing), "Various" → 2
    pub album_artist_variants: HashMap<String, usize>,
}

impl AlbumArtistIssue {
    /// Get the most common artist value (for pre-fill suggestions).
    pub fn most_common_artist(&self) -> Option<&str> {
        self.artist_variants
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(name, _)| name.as_str())
    }

    /// Get the most common album_artist value (may be empty string for missing).
    pub fn most_common_album_artist(&self) -> Option<&str> {
        self.album_artist_variants
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(name, _)| name.as_str())
    }

    /// Check if most tracks are missing album_artist.
    pub fn mostly_missing_album_artist(&self) -> bool {
        let missing_count = self.album_artist_variants.get("").copied().unwrap_or(0);
        let total: usize = self.album_artist_variants.values().sum();
        missing_count > total / 2
    }
}

/// Detect albums needing album_artist resolution.
///
/// Returns albums where:
/// - Multiple distinct artist values exist on tracks with the same album
/// - album_artist is missing OR inconsistent across those tracks
///
/// This is designed to catch:
/// - Compilations where each track has a different artist but album_artist is unset
/// - Multi-artist albums where album_artist should be unified
pub fn detect_inconsistent_album_artist(db: &Database) -> Result<Vec<AlbumArtistIssue>> {
    // Query: For each album, get all tracks with their artist and album_artist values
    // Group by normalized album, filter to those with multiple distinct artists
    // and missing/inconsistent album_artist

    let mut issues = Vec::new();

    // Get all albums with their track tags
    // We need: album, artist, album_artist for each track
    let album_data = query_album_artist_data(db)?;

    for (normalized_album, tracks) in album_data {
        // Count distinct artists
        let mut artist_counts: HashMap<String, usize> = HashMap::new();
        let mut album_artist_counts: HashMap<String, usize> = HashMap::new();
        let mut track_ids = Vec::new();
        let mut album_name = String::new();

        // Collect distinct catalog numbers and ISRCs (excluding empty values)
        let mut catalog_numbers: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let mut isrcs: std::collections::HashSet<&str> = std::collections::HashSet::new();

        for track in &tracks {
            track_ids.push(track.track_id);
            *artist_counts.entry(track.artist.clone()).or_insert(0) += 1;
            *album_artist_counts.entry(track.album_artist.clone()).or_insert(0) += 1;
            if album_name.is_empty() && !track.album.is_empty() {
                album_name = track.album.clone();
            }
            // Track non-empty catalog numbers and ISRCs
            if !track.catalog_number.is_empty() {
                catalog_numbers.insert(&track.catalog_number);
            }
            if !track.isrc.is_empty() {
                isrcs.insert(&track.isrc);
            }
        }

        // Skip if only one artist (not a multi-artist album)
        if artist_counts.len() < 2 {
            continue;
        }

        // Skip if tracks have different catalog numbers - indicates different releases
        // (e.g., "Surge" and "Surge EP" from different artists with different catalog numbers)
        if catalog_numbers.len() > 1 {
            continue;
        }

        // Skip if tracks have different ISRCs - indicates different releases
        if isrcs.len() > 1 {
            continue;
        }

        // Check if album_artist is inconsistent or missing
        // Missing = empty string is common; inconsistent = multiple different values
        let has_missing = album_artist_counts.contains_key("");
        let has_multiple = album_artist_counts.len() > 1;

        if has_missing || has_multiple {
            issues.push(AlbumArtistIssue {
                album: album_name,
                normalized_album,
                track_ids,
                artist_variants: artist_counts,
                album_artist_variants: album_artist_counts,
            });
        }
    }

    Ok(issues)
}

/// Track data collected per album for analysis.
struct TrackAlbumData {
    track_id: i64,
    artist: String,
    album_artist: String,
    album: String,
    catalog_number: String,
    isrc: String,
}

/// Query album/artist/album_artist data for all tracks.
/// Also fetches catalog_number and isrc for release differentiation.
/// Returns: HashMap<normalized_album, Vec<TrackAlbumData>>
fn query_album_artist_data(
    db: &Database,
) -> Result<HashMap<String, Vec<TrackAlbumData>>> {
    // SQL to get tracks with album, artist, album_artist, catalog_number, isrc tags
    // Using LEFT JOINs to handle missing tags
    let sql = r#"
        SELECT
            t.id as track_id,
            COALESCE(album.tag_value, '') as album,
            COALESCE(artist.tag_value, '') as artist,
            COALESCE(album_artist.tag_value, '') as album_artist,
            COALESCE(catalog.tag_value, '') as catalog_number,
            COALESCE(isrc.tag_value, '') as isrc
        FROM tracks t
        LEFT JOIN track_tags album
            ON t.id = album.track_id AND LOWER(album.tag_name) = 'album'
        LEFT JOIN track_tags artist
            ON t.id = artist.track_id AND LOWER(artist.tag_name) = 'artist'
        LEFT JOIN track_tags album_artist
            ON t.id = album_artist.track_id AND LOWER(album_artist.tag_name) = 'album_artist'
        LEFT JOIN track_tags catalog
            ON t.id = catalog.track_id AND LOWER(catalog.tag_name) = 'catalognumber'
        LEFT JOIN track_tags isrc
            ON t.id = isrc.track_id AND LOWER(isrc.tag_name) = 'isrc'
        WHERE album.tag_value IS NOT NULL AND album.tag_value != ''
    "#;

    let mut stmt = db.conn.prepare(sql)?;
    let rows = stmt.query_map([], |row| {
        Ok(TrackAlbumData {
            track_id: row.get(0)?,
            album: row.get(1)?,
            artist: row.get(2)?,
            album_artist: row.get(3)?,
            catalog_number: row.get(4)?,
            isrc: row.get(5)?,
        })
    })?;

    let mut album_data: HashMap<String, Vec<TrackAlbumData>> = HashMap::new();

    for row in rows {
        let data = row?;
        let normalized = normalize_album(&data.album);
        album_data.entry(normalized).or_default().push(data);
    }

    Ok(album_data)
}

/// Normalize an album name for grouping.
fn normalize_album(album: &str) -> String {
    // Use artist normalization as a base (lowercasing, trimming)
    // Albums need less aggressive normalization than artists
    normalize_artist(album)
}

