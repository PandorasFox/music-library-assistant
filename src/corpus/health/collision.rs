//! Tag Collision Detection
//!
//! Functions for detecting and reporting tag value collisions from a TagCloud.
//! Provides both full collision details for triage UI and quick boolean checks
//! for health indexing.

use std::collections::HashMap;

use super::tag_cloud::TagCloud;

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
// Core collision detection (returns full collision info for triage)
// ============================================================================

/// Get all artist name collisions from the tag cloud.
///
/// Example: "nervous_testpilot" vs "Nervous Testpilot" -> collision
pub fn get_artist_collisions(cloud: &TagCloud) -> Vec<TagCollision> {
    cloud
        .artist_keys()
        .filter_map(|key| {
            let variants = cloud.artist_variants(key)?;
            if variants.len() > 1 {
                Some(TagCollision::from_variants("artist", key, variants))
            } else {
                None
            }
        })
        .collect()
}

/// Get all album_artist collisions from the tag cloud.
pub fn get_album_artist_collisions(cloud: &TagCloud) -> Vec<TagCollision> {
    cloud
        .album_artist_keys()
        .filter_map(|key| {
            let variants = cloud.album_artist_variants(key)?;
            if variants.len() > 1 {
                Some(TagCollision::from_variants("album_artist", key, variants))
            } else {
                None
            }
        })
        .collect()
}

/// Get album collisions, ONLY for albums with same artist/album_artist context.
///
/// Different artists with same album name are NOT collisions.
/// This prevents false positives like "Greatest Hits" by different artists.
pub fn get_album_collisions(cloud: &TagCloud) -> Vec<TagCollision> {
    cloud
        .album_keys()
        .filter_map(|(artist_context, normalized_album)| {
            let variants = cloud.album_variants(artist_context, normalized_album)?;
            if variants.len() > 1 {
                // Include artist context in the key for clarity
                let key = format!("{} :: {}", artist_context, normalized_album);
                Some(TagCollision::from_variants("album", &key, variants))
            } else {
                None
            }
        })
        .collect()
}

/// Get genre collisions from the tag cloud.
///
/// Example: "Hip Hop" vs "Hip-Hop" vs "HipHop" -> collision
pub fn get_genre_collisions(cloud: &TagCloud) -> Vec<TagCollision> {
    cloud
        .genre_keys()
        .filter_map(|key| {
            let variants = cloud.genre_variants(key)?;
            if variants.len() > 1 {
                Some(TagCollision::from_variants("genre", key, variants))
            } else {
                None
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_test_cloud() -> TagCloud {
        let mut artists: HashMap<String, HashMap<String, usize>> = HashMap::new();

        // Artist collision: "nervous_testpilot" vs "Nervous Testpilot"
        let mut nervous = HashMap::new();
        nervous.insert("nervous_testpilot".to_string(), 3);
        nervous.insert("Nervous Testpilot".to_string(), 7);
        artists.insert("nervous testpilot".to_string(), nervous);

        // No collision - single variant
        let mut single = HashMap::new();
        single.insert("Artist A".to_string(), 5);
        artists.insert("artist a".to_string(), single);

        let mut genres: HashMap<String, HashMap<String, usize>> = HashMap::new();

        // Genre collision: "Hip Hop" vs "Hip-Hop"
        let mut hiphop = HashMap::new();
        hiphop.insert("Hip Hop".to_string(), 10);
        hiphop.insert("Hip-Hop".to_string(), 5);
        genres.insert("hip-hop".to_string(), hiphop);

        TagCloud::new_test(
            artists,
            HashMap::new(), // album_artists
            HashMap::new(), // albums
            genres,
            25, // track_count
        )
    }

    #[test]
    fn test_get_artist_collisions() {
        let cloud = make_test_cloud();
        let collisions = get_artist_collisions(&cloud);

        assert_eq!(collisions.len(), 1);
        let collision = &collisions[0];
        assert_eq!(collision.tag_name, "artist");
        assert_eq!(collision.variants.len(), 2);
        assert_eq!(collision.canonical, "Nervous Testpilot"); // More common
        assert!(collision.confidence > 0.5); // 7 out of 10
    }

    #[test]
    fn test_get_genre_collisions() {
        let cloud = make_test_cloud();
        let collisions = get_genre_collisions(&cloud);

        assert_eq!(collisions.len(), 1);
        let collision = &collisions[0];
        assert_eq!(collision.tag_name, "genre");
        assert_eq!(collision.canonical, "Hip Hop"); // More common
    }

}
