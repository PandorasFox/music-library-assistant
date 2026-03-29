//! Tag Collision Detection
//!
//! Detects tag value collisions (multiple spellings that normalize to the same key)
//! using efficient SQL queries against the corpus_tags table.

use std::collections::{HashMap, HashSet};

use anyhow::Result;

use super::normalization::{
    normalize_album, normalize_album_artist, normalize_artist, normalize_genre,
};
use crate::db::ReadOnlyDb;

/// A detected collision between tag values.
#[derive(Debug, Clone)]
pub struct TagCollision {
    /// Tag field name: "artist", "album_artist", "album", "genre"
    pub tag_name: String,
    /// The normalized key that caused the collision
    pub normalized_key: String,
    /// Original values that collide (multiple spellings)
    pub variants: Vec<String>,
    /// Count per variant (includes all corpus files; non-MB counts are
    /// recomputed in the canonicity computation).
    pub _variant_counts: HashMap<String, usize>,
    /// Suggested canonical value (most common spelling)
    pub _canonical: String,
    /// Confidence score (ratio of canonical count to total)
    pub _confidence: f64,
}

impl TagCollision {
    /// Create from variant counts map.
    fn from_variants(
        tag_name: &str,
        normalized_key: &str,
        variants: &HashMap<String, usize>,
    ) -> Self {
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
            _variant_counts: variants.clone(),
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
    let values = db.get_distinct_tag_values_for::<crate::zones::CorpusZone>("artist")?;

    // Group by normalized key
    let mut buckets: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (value, count) in values {
        let normalized = normalize_artist(&value);
        buckets.entry(normalized).or_default().insert(value, count);
    }

    // Convert buckets with multiple variants to collisions
    Ok(buckets
        .into_iter()
        .filter(|(_, variants)| variants.len() > 1)
        .map(|(key, variants)| TagCollision::from_variants("artist", &key, &variants))
        .collect())
}

/// Detect album_artist collisions from the database.
///
/// Queries all compound tag name variants (ALBUMARTIST, ALBUM_ARTIST, ALBUM ARTIST)
/// and merges results, since different files may use different separator conventions.
pub fn get_album_artist_collisions(db: &ReadOnlyDb<'_>) -> Result<Vec<TagCollision>> {
    // Query all separator variants and merge — files may use ALBUMARTIST or ALBUM_ARTIST
    let mut buckets: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for variant in mm_utils::tag_names::compound_tag_name_variants("ALBUM", "ARTIST") {
        let values = db.get_distinct_tag_values_for::<crate::zones::CorpusZone>(&variant)?;
        for (value, count) in values {
            let normalized = normalize_album_artist(&value);
            *buckets
                .entry(normalized)
                .or_default()
                .entry(value)
                .or_insert(0) += count;
        }
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
/// When `strip_format_suffixes` is true, EP/LP suffixes are stripped so
/// "Album EP" and "Album" normalize to the same key. When false, they are distinct.
///
/// Additionally, variants with disjoint ISRCs or catalog numbers are considered
/// distinct releases and NOT collisions (e.g., "Album EP" with ISRCs {A,B,C} and
/// "Album" with ISRCs {D,E,F,G} are different releases, not canonicalization issues).
pub fn get_album_collisions(
    db: &ReadOnlyDb<'_>,
    strip_format_suffixes: bool,
    mb_release_tag_name: &str,
) -> Result<Vec<TagCollision>> {
    let rows = db.get_album_data_for_collision_detection(mb_release_tag_name)?;

    // Group by (normalized_artist, normalized_album)
    // For each group, track: variant -> (count, isrcs, catalog_numbers)
    let mut buckets: HashMap<(String, String), HashMap<String, VariantData>> = HashMap::new();

    for (album, artist_context, isrc, catalog_number, year, date, mb_release_id) in rows {
        let normalized_artist = normalize_artist(&artist_context);
        let normalized_album = normalize_album(&album, strip_format_suffixes);
        let key = (normalized_artist, normalized_album);

        let variant_data = buckets.entry(key).or_default().entry(album).or_default();

        variant_data.count += 1;
        if !isrc.is_empty() {
            variant_data.isrcs.insert(isrc);
        }
        if !catalog_number.is_empty() {
            variant_data.catalog_numbers.insert(catalog_number);
        }
        if !mb_release_id.is_empty() {
            variant_data.mb_release_ids.insert(mb_release_id);
        }
        // Extract release year from either `year` tag or leading 4 digits of `date` tag
        let release_year = extract_release_year(&year, &date);
        if let Some(y) = release_year {
            variant_data.years.insert(y);
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
#[derive(Default)]
struct VariantData {
    count: usize,
    isrcs: HashSet<String>,
    catalog_numbers: HashSet<String>,
    /// MusicBrainz release IDs (configured tag name, default MUSICBRAINZ_RELEASE).
    mb_release_ids: HashSet<String>,
    /// Release years extracted from `year` and/or `date` tags.
    years: HashSet<u16>,
}

/// Extract a 4-digit release year from `year` and/or `date` tag values.
///
/// Prefers `year` if it looks like a valid 4-digit year, otherwise tries
/// the leading 4 characters of `date` (e.g. "2012-10-23" → 2012).
fn extract_release_year(year_tag: &str, date_tag: &str) -> Option<u16> {
    // Try `year` tag first
    if year_tag.len() >= 4 {
        if let Ok(y) = year_tag[..4].parse::<u16>() {
            if (1900..2200).contains(&y) {
                return Some(y);
            }
        }
    }
    // Fall back to leading 4 chars of `date` tag
    if date_tag.len() >= 4 {
        if let Ok(y) = date_tag[..4].parse::<u16>() {
            if (1900..2200).contains(&y) {
                return Some(y);
            }
        }
    }
    None
}

/// Check if album variants have disjoint release identifiers.
///
/// Returns true if the variants are distinct releases (no collision),
/// i.e., ALL variants have non-empty, non-overlapping identifier sets.
///
/// Returns false (collision) if:
/// - Any variant has no identifiers (can't distinguish)
/// - Variants share identifiers (same release, different naming)
///
/// Check order: MB release ID (strongest), ISRC, catalog number, year.
fn variants_have_disjoint_release_ids(variants: &HashMap<String, VariantData>) -> bool {
    let variant_list: Vec<_> = variants.values().collect();

    // Check MusicBrainz release ID disjointness (strongest signal)
    let all_have_mb = variant_list.iter().all(|v| !v.mb_release_ids.is_empty());
    if all_have_mb {
        for (i, v1) in variant_list.iter().enumerate() {
            for v2 in variant_list.iter().skip(i + 1) {
                if !v1.mb_release_ids.is_disjoint(&v2.mb_release_ids) {
                    return false;
                }
            }
        }
        return true;
    }

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

    // Check release year disjointness - ALL variants must have years and be pairwise disjoint
    let all_have_years = variant_list.iter().all(|v| !v.years.is_empty());
    if all_have_years {
        for (i, v1) in variant_list.iter().enumerate() {
            for v2 in variant_list.iter().skip(i + 1) {
                if !v1.years.is_disjoint(&v2.years) {
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
    let values = db.get_distinct_tag_values_for::<crate::zones::CorpusZone>("genre")?;

    // Group by normalized key
    let mut buckets: HashMap<String, HashMap<String, usize>> = HashMap::new();
    for (value, count) in values {
        let normalized = normalize_genre(&value);
        buckets.entry(normalized).or_default().insert(value, count);
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
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
                ..Default::default()
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                ..Default::default()
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
                catalog_numbers: ["CAT001"].iter().map(|s| s.to_string()).collect(),
                ..Default::default()
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                catalog_numbers: ["CAT002"].iter().map(|s| s.to_string()).collect(),
                ..Default::default()
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
                ..Default::default()
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                ..Default::default()
            },
        );

        // Since not all variants have ISRCs, we can't distinguish - treat as collision
        assert!(!variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_disjoint_years_are_distinct_releases() {
        // "Hotline Miami" (2012) vs "Hotline Miami EP" (2013) - different releases
        let mut variants = HashMap::new();
        variants.insert(
            "Hotline Miami".to_string(),
            VariantData {
                count: 10,
                years: [2012].into_iter().collect(),
                ..Default::default()
            },
        );
        variants.insert(
            "Hotline Miami EP".to_string(),
            VariantData {
                count: 4,
                years: [2013].into_iter().collect(),
                ..Default::default()
            },
        );

        assert!(variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_same_year_is_collision() {
        // Same year, no other identifiers - still a collision
        let mut variants = HashMap::new();
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 3,
                years: [2020].into_iter().collect(),
                ..Default::default()
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                years: [2020].into_iter().collect(),
                ..Default::default()
            },
        );

        assert!(!variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_one_variant_missing_year_is_collision() {
        // Only one variant has year - can't distinguish
        let mut variants = HashMap::new();
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 3,
                years: [2020].into_iter().collect(),
                ..Default::default()
            },
        );
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 6,
                ..Default::default()
            },
        );

        assert!(!variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_extract_release_year_from_year_tag() {
        assert_eq!(extract_release_year("2012", ""), Some(2012));
        assert_eq!(extract_release_year("1985", ""), Some(1985));
        assert_eq!(extract_release_year("", ""), None);
        assert_eq!(extract_release_year("bad", ""), None);
    }

    #[test]
    fn test_extract_release_year_from_date_tag() {
        assert_eq!(extract_release_year("", "2013-06-15"), Some(2013));
        assert_eq!(extract_release_year("", "20130615"), Some(2013));
    }

    #[test]
    fn test_extract_release_year_prefers_year_tag() {
        // year tag wins when both present
        assert_eq!(extract_release_year("2012", "2013-06-15"), Some(2012));
    }

    #[test]
    fn test_disjoint_mb_release_ids_are_distinct_releases() {
        // Different MUSICBRAINZ_ALBUMID values — strongest disjointness signal
        let mut variants = HashMap::new();
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 10,
                mb_release_ids: ["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Default::default()
            },
        );
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 4,
                mb_release_ids: ["11111111-2222-3333-4444-555555555555"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Default::default()
            },
        );

        assert!(variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_overlapping_mb_release_ids_are_collision() {
        // Same MUSICBRAINZ_ALBUMID shared between variants = same release
        let mbid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string();
        let mut variants = HashMap::new();
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 10,
                mb_release_ids: [mbid.clone()].into_iter().collect(),
                ..Default::default()
            },
        );
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 4,
                mb_release_ids: [mbid].into_iter().collect(),
                ..Default::default()
            },
        );

        assert!(!variants_have_disjoint_release_ids(&variants));
    }

    #[test]
    fn test_mb_release_id_checked_before_isrc() {
        // MB release IDs disjoint should return true even if ISRCs overlap
        // (MB release ID is the strongest signal, checked first)
        let shared_isrc = "USXXXX1234567".to_string();
        let mut variants = HashMap::new();
        variants.insert(
            "Album".to_string(),
            VariantData {
                count: 10,
                isrcs: [shared_isrc.clone()].into_iter().collect(),
                mb_release_ids: ["aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Default::default()
            },
        );
        variants.insert(
            "Album EP".to_string(),
            VariantData {
                count: 4,
                isrcs: [shared_isrc].into_iter().collect(),
                mb_release_ids: ["11111111-2222-3333-4444-555555555555"]
                    .iter()
                    .map(|s| s.to_string())
                    .collect(),
                ..Default::default()
            },
        );

        // MB release IDs are disjoint, so this returns true (distinct releases)
        // even though ISRCs overlap
        assert!(variants_have_disjoint_release_ids(&variants));
    }
}
