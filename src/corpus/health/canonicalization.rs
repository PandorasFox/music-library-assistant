//! Tag Canonicalization Detection
//!
//! Uses TagCloud for efficient collision detection with bespoke
//! normalization per tag type.

use anyhow::Result;

use super::collision::{
    get_album_artist_collisions, get_album_collisions, get_artist_collisions,
    get_genre_collisions, TagCollision,
};
use super::tag_cloud::TagCloud;
use crate::config;
use crate::corpus::db::types::TagCanonicalization;
use crate::corpus::db::Database;

/// Detect and store all canonicalization issues.
/// Called during eyeballing or post-scan.
/// Returns the count of new canonicalization entries added.
pub fn detect_and_store_canonicalizations(db: &Database) -> Result<usize> {
    let cloud = TagCloud::build(db)?;
    let mut total_new = 0;

    // Artist collisions
    for collision in get_artist_collisions(&cloud) {
        total_new += store_collision(db, &collision)?;
    }

    // Album artist collisions
    for collision in get_album_artist_collisions(&cloud) {
        total_new += store_collision(db, &collision)?;
    }

    // Album collisions (context-aware - requires same artist)
    for collision in get_album_collisions(&cloud) {
        total_new += store_collision(db, &collision)?;
    }

    // Genre collisions
    for collision in get_genre_collisions(&cloud) {
        total_new += store_collision(db, &collision)?;
    }

    if total_new > 0 {
        let _ = config::log_message(&format!(
            "Tag canonicalization: detected {} new issues (artists: {}, album_artists: {}, albums: {}, genres: {})",
            total_new,
            cloud.artist_collision_count(),
            cloud.album_artist_collision_count(),
            cloud.album_collision_count(),
            cloud.genre_collision_count(),
        ));
    }

    Ok(total_new)
}

/// Detect and store canonicalization issues using an existing TagCloud.
/// Use this when you already have a TagCloud built to avoid rebuilding.
pub fn detect_and_store_canonicalizations_with_cloud(
    db: &Database,
    cloud: &TagCloud,
) -> Result<usize> {
    let mut total_new = 0;

    for collision in get_artist_collisions(cloud) {
        total_new += store_collision(db, &collision)?;
    }

    for collision in get_album_artist_collisions(cloud) {
        total_new += store_collision(db, &collision)?;
    }

    for collision in get_album_collisions(cloud) {
        total_new += store_collision(db, &collision)?;
    }

    for collision in get_genre_collisions(cloud) {
        total_new += store_collision(db, &collision)?;
    }

    Ok(total_new)
}

/// Store a single collision's variants in the database.
/// Returns count of new entries added.
fn store_collision(db: &Database, collision: &TagCollision) -> Result<usize> {
    let mut new_count = 0;

    for variant in &collision.variants {
        // Skip the canonical value itself
        if variant == &collision.canonical {
            continue;
        }

        // Check if this variant is already stored
        if db.get_canonical_tag_value(&collision.tag_name, variant)?.is_some() {
            continue;
        }

        let canon_entry = TagCanonicalization {
            id: None,
            tag_name: collision.tag_name.clone(),
            canonical_value: collision.canonical.clone(),
            variant_value: variant.clone(),
            confidence: Some(collision.confidence),
            auto_detected: true,
            confirmed_at: None,
        };

        db.upsert_tag_canonicalization(&canon_entry)?;
        new_count += 1;

        let _ = config::log_message(&format!(
            "[{}] Canonicalization: '{}' -> '{}' (confidence: {:.2})",
            collision.tag_name, variant, collision.canonical, collision.confidence
        ));
    }

    Ok(new_count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_collision_skips_canonical() {
        // This would need a test database to fully test
        // For now, just verify the module compiles
    }
}
