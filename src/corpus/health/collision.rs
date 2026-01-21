//! Tag Collision Detection
//!
//! Detects tag value collisions (multiple spellings that normalize to the same key)
//! using efficient SQL queries against the track_tags table.

use std::collections::HashMap;

use anyhow::Result;

use super::normalization::{normalize_album, normalize_album_artist, normalize_artist, normalize_genre};
use crate::corpus::db::Database;

/// A detected collision between tag values.
#[derive(Debug, Clone)]
pub struct TagCollision {
    /// Tag field name: "artist", "album_artist", "album", "genre"
    pub tag_name: String,
    /// The normalized key that caused the collision
    pub normalized_key: String,
    /// Original values that collide (multiple spellings)
    pub variants: Vec<String>,
    /// Count per variant
    pub variant_counts: HashMap<String, usize>,
    /// Suggested canonical value (most common spelling)
    pub canonical: String,
    /// Confidence score (ratio of canonical count to total)
    pub confidence: f64,
}

impl TagCollision {
    /// Create from variant counts map.
    fn from_variants(tag_name: &str, normalized_key: &str, variants: &HashMap<String, usize>) -> Self {
        let total: usize = variants.values().sum();

        // Find canonical (most common)
        let canonical = variants
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(name, _)| name.clone())
            .unwrap_or_default();

        let canonical_count = variants.get(&canonical).copied().unwrap_or(0);
        let confidence = if total > 0 {
            canonical_count as f64 / total as f64
        } else {
            0.0
        };

        TagCollision {
            tag_name: tag_name.to_string(),
            normalized_key: normalized_key.to_string(),
            variants: variants.keys().cloned().collect(),
            variant_counts: variants.clone(),
            canonical,
            confidence,
        }
    }
}

// ============================================================================
// Collision detection using database queries
// ============================================================================

/// Detect artist name collisions from the database.
pub fn get_artist_collisions(db: &Database) -> Result<Vec<TagCollision>> {
    let values = db.get_distinct_tag_values("artist")?;

    // Group by normalized key
    let mut buckets: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (value, count) in values {
        let normalized = normalize_artist(&value);
        buckets
            .entry(normalized)
            .or_default()
            .insert(value, count);
    }

    // Convert buckets with multiple variants to collisions
    Ok(buckets
        .into_iter()
        .filter(|(_, variants)| variants.len() > 1)
        .map(|(key, variants)| TagCollision::from_variants("artist", &key, &variants))
        .collect())
}

/// Detect album_artist collisions from the database.
pub fn get_album_artist_collisions(db: &Database) -> Result<Vec<TagCollision>> {
    let values = db.get_distinct_tag_values("album_artist")?;

    // Group by normalized key
    let mut buckets: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (value, count) in values {
        let normalized = normalize_album_artist(&value);
        buckets
            .entry(normalized)
            .or_default()
            .insert(value, count);
    }

    // Convert buckets with multiple variants to collisions
    Ok(buckets
        .into_iter()
        .filter(|(_, variants)| variants.len() > 1)
        .map(|(key, variants)| TagCollision::from_variants("album_artist", &key, &variants))
        .collect())
}

/// Detect album collisions from the database.
///
/// Albums are keyed by artist context to avoid false positives
/// (e.g., "Greatest Hits" by different artists are NOT collisions).
pub fn get_album_collisions(db: &Database) -> Result<Vec<TagCollision>> {
    let values = db.get_album_values_with_artist_context()?;

    // Group by (normalized_artist, normalized_album)
    let mut buckets: HashMap<(String, String), HashMap<String, usize>> = HashMap::new();
    for (album, artist_context, count) in values {
        let normalized_artist = normalize_artist(&artist_context);
        let normalized_album = normalize_album(&album);
        let key = (normalized_artist, normalized_album);
        buckets
            .entry(key)
            .or_default()
            .insert(album, count);
    }

    // Convert buckets with multiple variants to collisions
    Ok(buckets
        .into_iter()
        .filter(|(_, variants)| variants.len() > 1)
        .map(|((artist_ctx, album_key), variants)| {
            let key = format!("{} :: {}", artist_ctx, album_key);
            TagCollision::from_variants("album", &key, &variants)
        })
        .collect())
}

/// Detect genre collisions from the database.
pub fn get_genre_collisions(db: &Database) -> Result<Vec<TagCollision>> {
    let values = db.get_distinct_tag_values("genre")?;

    // Group by normalized key
    let mut buckets: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (value, count) in values {
        let normalized = normalize_genre(&value);
        buckets
            .entry(normalized)
            .or_default()
            .insert(value, count);
    }

    // Convert buckets with multiple variants to collisions
    Ok(buckets
        .into_iter()
        .filter(|(_, variants)| variants.len() > 1)
        .map(|(key, variants)| TagCollision::from_variants("genre", &key, &variants))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collision_from_variants() {
        let mut variants = HashMap::new();
        variants.insert("nervous_testpilot".to_string(), 3);
        variants.insert("Nervous Testpilot".to_string(), 7);

        let collision = TagCollision::from_variants("artist", "nervous testpilot", &variants);

        assert_eq!(collision.tag_name, "artist");
        assert_eq!(collision.variants.len(), 2);
        assert_eq!(collision.canonical, "Nervous Testpilot"); // More common
        assert!((collision.confidence - 0.7).abs() < 0.01); // 7 out of 10
    }

    #[test]
    fn test_collision_confidence() {
        let mut variants = HashMap::new();
        variants.insert("Hip Hop".to_string(), 10);
        variants.insert("Hip-Hop".to_string(), 5);
        variants.insert("HipHop".to_string(), 5);

        let collision = TagCollision::from_variants("genre", "hip-hop", &variants);

        assert_eq!(collision.canonical, "Hip Hop");
        assert!((collision.confidence - 0.5).abs() < 0.01); // 10 out of 20
    }
}
