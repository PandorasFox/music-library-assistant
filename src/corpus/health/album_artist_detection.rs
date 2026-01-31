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
    let rows = db.get_album_artist_data()?;

    let mut album_data: HashMap<String, Vec<TrackAlbumData>> = HashMap::new();

    for (track_id, album, artist, album_artist, catalog_number, isrc) in rows {
        let data = TrackAlbumData {
            track_id,
            album: album.clone(),
            artist,
            album_artist,
            catalog_number,
            isrc,
        };
        let normalized = normalize_album(&album);
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

