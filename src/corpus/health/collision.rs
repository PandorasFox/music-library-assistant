//! Tag Collision Detection
//!
//! Detects tag value collisions (multiple spellings that normalize to the same key)
//! using efficient SQL queries against the corpus_tags table.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::normalization::{normalize_album, normalize_album_artist, normalize_artist, normalize_genre};
use crate::corpus::db::ReadOnlyDb;

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
    pub _canonical: String,
    /// Confidence score (ratio of canonical count to total)
    pub _confidence: f64,
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
            _canonical: canonical,
            _confidence: confidence,
        }
    }
}

// ============================================================================
// Collision detection using database queries
// ============================================================================

/// Detect artist name collisions from the database.
pub fn get_artist_collisions(db: &ReadOnlyDb<'_>) -> Result<Vec<TagCollision>> {
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
pub fn get_album_artist_collisions(db: &ReadOnlyDb<'_>) -> Result<Vec<TagCollision>> {
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
///
/// Additionally, variants with disjoint ISRCs or catalog numbers are considered
/// distinct releases and NOT collisions (e.g., "Album EP" with ISRCs {A,B,C} and
/// "Album" with ISRCs {D,E,F,G} are different releases, not canonicalization issues).
pub fn get_album_collisions(db: &ReadOnlyDb<'_>) -> Result<Vec<TagCollision>> {
    let rows = db.get_album_data_for_collision_detection()?;

    // Group by (normalized_artist, normalized_album)
    // For each group, track: variant -> (count, isrcs, catalog_numbers)
    let mut buckets: HashMap<(String, String), HashMap<String, VariantData>> = HashMap::new();

    for (album, artist_context, isrc, catalog_number) in rows {
        let normalized_artist = normalize_artist(&artist_context);
        let normalized_album = normalize_album(&album);
        let key = (normalized_artist, normalized_album);

        let variant_data = buckets
            .entry(key)
            .or_default()
            .entry(album)
            .or_insert_with(VariantData::default);

        variant_data.count += 1;
        if !isrc.is_empty() {
            variant_data.isrcs.insert(isrc);
        }
        if !catalog_number.is_empty() {
            variant_data.catalog_numbers.insert(catalog_number);
        }
    }

    // Convert buckets with multiple variants to collisions,
    // filtering out those where variants have disjoint release identifiers
    Ok(buckets
        .into_iter()
        .filter(|(_, variants)| variants.len() > 1)
        .filter(|(_, variants)| !variants_have_disjoint_release_ids(variants))
        .map(|((artist_ctx, album_key), variants)| {
            let key = format!("{} :: {}", artist_ctx, album_key);
            let counts: HashMap<String, usize> = variants
                .into_iter()
                .map(|(name, data)| (name, data.count))
                .collect();
            TagCollision::from_variants("album", &key, &counts)
        })
        .collect())
}

/// Data collected per album variant for collision detection.
///
/// TODO: Expand to include other release identifiers when we parse them:
/// - MusicBrainz release ID (MUSICBRAINZ_ALBUMID)
/// - MusicBrainz release group ID (MUSICBRAINZ_RELEASEGROUPID)
/// - Discogs release ID (DISCOGS_RELEASE_ID)
/// - Barcode/UPC
#[derive(Default)]
struct VariantData {
    count: usize,
    isrcs: HashSet<String>,
    catalog_numbers: HashSet<String>,
}

/// Check if album variants have disjoint release identifiers.
///
/// Returns true if the variants are distinct releases (no collision),
/// i.e., ALL variants have non-empty, non-overlapping ISRC or catalog number sets.
///
/// Returns false (collision) if:
/// - Any variant has no identifiers (can't distinguish)
/// - Variants share ISRCs or catalog numbers (same release, different naming)
///
/// TODO: When we add support for additional release identifiers (MusicBrainz,
/// Discogs, barcode), check those here as well. A single disjoint identifier
/// type should be sufficient to distinguish releases.
fn variants_have_disjoint_release_ids(variants: &HashMap<String, VariantData>) -> bool {
    let variant_list: Vec<_> = variants.values().collect();

    // Check ISRC disjointness - ALL variants must have ISRCs and be pairwise disjoint
    let all_have_isrcs = variant_list.iter().all(|v| !v.isrcs.is_empty());
    if all_have_isrcs {
        for (i, v1) in variant_list.iter().enumerate() {
            for v2 in variant_list.iter().skip(i + 1) {
                // If any two variants share an ISRC, they're not disjoint
                if !v1.isrcs.is_disjoint(&v2.isrcs) {
                    return false;
                }
            }
        }
        // All variants have ISRCs and are pairwise disjoint
        return true;
    }

    // Check catalog number disjointness - ALL variants must have catalog numbers
    let all_have_catalogs = variant_list.iter().all(|v| !v.catalog_numbers.is_empty());
    if all_have_catalogs {
        for (i, v1) in variant_list.iter().enumerate() {
            for v2 in variant_list.iter().skip(i + 1) {
                if !v1.catalog_numbers.is_disjoint(&v2.catalog_numbers) {
                    return false;
                }
            }
        }
        return true;
    }

    // Not all variants have identifiers - can't conclusively distinguish, treat as collision
    false
}

/// Detect genre collisions from the database.
pub fn get_genre_collisions(db: &ReadOnlyDb<'_>) -> Result<Vec<TagCollision>> {
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
        assert_eq!(collision._canonical, "Nervous Testpilot"); // More common
        assert!((collision._confidence - 0.7).abs() < 0.01); // 7 out of 10
    }

    #[test]
    fn test_collision_confidence() {
        let mut variants = HashMap::new();
        variants.insert("Hip Hop".to_string(), 10);
        variants.insert("Hip-Hop".to_string(), 5);
        variants.insert("HipHop".to_string(), 5);

        let collision = TagCollision::from_variants("genre", "hip-hop", &variants);

        assert_eq!(collision._canonical, "Hip Hop");
        assert!((collision._confidence - 0.5).abs() < 0.01); // 10 out of 20
    }

    #[test]
    fn test_disjoint_isrcs_are_distinct_releases() {
        // "Album EP" with ISRCs {A, B, C} and "Album" with ISRCs {D, E, F}
        // These are distinct releases, NOT a collision
        let mut variants = HashMap::new();
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 3,
                isrcs: ["ISRC_A", "ISRC_B", "ISRC_C"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                catalog_numbers: HashSet::new(),
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                isrcs: ["ISRC_D", "ISRC_E", "ISRC_F"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                catalog_numbers: HashSet::new(),
            },
        );

        assert!(variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_overlapping_isrcs_are_collision() {
        // "Album EP" and "Album" share ISRC_A - same release, naming inconsistency
        let mut variants = HashMap::new();
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 3,
                isrcs: ["ISRC_A", "ISRC_B", "ISRC_C"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                catalog_numbers: HashSet::new(),
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                isrcs: ["ISRC_A", "ISRC_D", "ISRC_E"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                catalog_numbers: HashSet::new(),
            },
        );

        assert!(!variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_no_isrcs_is_collision() {
        // Neither variant has ISRCs - can't distinguish, treat as collision
        let mut variants = HashMap::new();
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 3,
                isrcs: HashSet::new(),
                catalog_numbers: HashSet::new(),
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                isrcs: HashSet::new(),
                catalog_numbers: HashSet::new(),
            },
        );

        assert!(!variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_disjoint_catalog_numbers_are_distinct_releases() {
        // Different catalog numbers, no ISRCs - distinct releases
        let mut variants = HashMap::new();
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 3,
                isrcs: HashSet::new(),
                catalog_numbers: ["CAT001"].iter().map(|s| s.to_string()).collect(),
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                isrcs: HashSet::new(),
                catalog_numbers: ["CAT002"].iter().map(|s| s.to_string()).collect(),
            },
        );

        assert!(variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_one_variant_has_isrcs_other_doesnt() {
        // Only one variant has ISRCs - we can't conclusively say they're different
        // since the variant without ISRCs could be from the same release
        let mut variants = HashMap::new();
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 3,
                isrcs: ["ISRC_A", "ISRC_B", "ISRC_C"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                catalog_numbers: HashSet::new(),
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                isrcs: HashSet::new(),
                catalog_numbers: HashSet::new(),
            },
        );

        // Since not all variants have ISRCs, we can't distinguish - treat as collision
        assert!(!variants_have_disjoint_release_ids(&variants));
    }
}
