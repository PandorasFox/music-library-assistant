//! Tag Canonicalization Detection
//!
//! Uses SQL-based collision detection for efficient discovery of
//! tag value variants that should be unified.

use anyhow::Result;

use super::collision::{
    get_album_artist_collisions, get_album_collisions, get_artist_collisions,
    get_genre_collisions, TagCollision,
};
use crate::config;
use crate::corpus::db::Database;

/// A detected canonicalization (not yet stored).
#[derive(Debug, Clone)]
pub struct DetectedCanonicalization {
    pub tag_name: String,
    pub canonical_value: String,
    pub variant_value: String,
    pub confidence: Option<f64>,
}

/// Detect all canonicalization issues (read-only, no storage).
/// Returns a list of detected canonicalizations that can be stored via db_thread.
pub fn detect_canonicalizations(read_only_db: &Database) -> Result<Vec<DetectedCanonicalization>> {
    let mut results = Vec::new();

    // Artist collisions
    for collision in get_artist_collisions(read_only_db)? {
        results.extend(collect_collision_entries(read_only_db, &collision)?);
    }

    // Album artist collisions
    for collision in get_album_artist_collisions(read_only_db)? {
        results.extend(collect_collision_entries(read_only_db, &collision)?);
    }

    // Album collisions (context-aware - requires same artist)
    for collision in get_album_collisions(read_only_db)? {
        results.extend(collect_collision_entries(read_only_db, &collision)?);
    }

    // Genre collisions
    for collision in get_genre_collisions(read_only_db)? {
        results.extend(collect_collision_entries(read_only_db, &collision)?);
    }

    if !results.is_empty() {
        let artist_count = get_artist_collisions(read_only_db).map(|c| c.len()).unwrap_or(0);
        let album_artist_count = get_album_artist_collisions(read_only_db).map(|c| c.len()).unwrap_or(0);
        let album_count = get_album_collisions(read_only_db).map(|c| c.len()).unwrap_or(0);
        let genre_count = get_genre_collisions(read_only_db).map(|c| c.len()).unwrap_or(0);

        let _ = config::log_message(&format!(
            "Tag canonicalization: detected {} issues (artists: {}, album_artists: {}, albums: {}, genres: {})",
            results.len(),
            artist_count,
            album_artist_count,
            album_count,
            genre_count,
        ));
    }

    Ok(results)
}

/// Collect canonicalization entries from a collision (read-only check).
fn collect_collision_entries(
    read_only_db: &Database,
    collision: &TagCollision,
) -> Result<Vec<DetectedCanonicalization>> {
    let mut entries = Vec::new();

    for variant in &collision.variants {
        // Skip the canonical value itself
        if variant == &collision.canonical {
            continue;
        }

        // Check if this variant is already stored (read-only check)
        if read_only_db
            .get_canonical_tag_value(&collision.tag_name, variant)?
            .is_some()
        {
            continue;
        }

        entries.push(DetectedCanonicalization {
            tag_name: collision.tag_name.clone(),
            canonical_value: collision.canonical.clone(),
            variant_value: variant.clone(),
            confidence: Some(collision.confidence),
        });

        let _ = config::log_message(&format!(
            "[{}] Canonicalization: '{}' -> '{}' (confidence: {:.2})",
            collision.tag_name, variant, collision.canonical, collision.confidence
        ));
    }

    Ok(entries)
}
