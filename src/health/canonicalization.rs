//! Artist Canonicalization Detection
//!
//! Detects variant spellings of artist names and stores them for resolution.
//! This runs as a post-scan action to populate the `artist_canonicalization` table.

use std::collections::HashMap;

use anyhow::Result;

use crate::db::types::ArtistCanonicalization;
use crate::db::Database;

/// Detect artist name variants and store them in the database.
/// Returns the count of new canonicalization entries added.
pub fn detect_and_store_canonicalizations(db: &Database) -> Result<usize> {
    // Get all corpus tracks
    let all_tracks = db.get_all_tracks(Some("corpus"))?;

    if all_tracks.is_empty() {
        return Ok(0);
    }

    // Build map of normalized artist name -> list of original spellings
    let mut artist_variants: HashMap<String, Vec<String>> = HashMap::new();

    for track in &all_tracks {
        if let Some(artist) = &track.artist {
            let normalized = normalize_for_comparison(artist);
            if !normalized.is_empty() {
                artist_variants
                    .entry(normalized)
                    .or_default()
                    .push(artist.clone());
            }
        }
    }

    // Find cases with multiple unique spellings
    let mut new_entries = 0;

    for (_normalized, variants) in artist_variants {
        // Get unique variants
        let unique: std::collections::HashSet<_> = variants.iter().collect();

        if unique.len() <= 1 {
            continue;
        }

        // Count occurrences of each variant
        let mut variant_counts: HashMap<&String, usize> = HashMap::new();
        for v in &variants {
            *variant_counts.entry(v).or_insert(0) += 1;
        }

        // The most common spelling becomes canonical
        let canonical = variant_counts
            .iter()
            .max_by_key(|(_, count)| *count)
            .map(|(name, _)| (*name).clone())
            .unwrap_or_else(|| variants[0].clone());

        // Insert all non-canonical variants
        for variant in unique {
            if variant == &canonical {
                continue;
            }

            // Check if this variant is already in the database
            if db.get_canonical_artist(variant)?.is_some() {
                continue;
            }

            let count = variant_counts.get(variant).unwrap_or(&0);
            let total = variants.len();

            // Calculate confidence based on ratio
            // Higher confidence when canonical is much more common
            let canonical_count = variant_counts.get(&canonical).unwrap_or(&0);
            let confidence = if total > 0 {
                *canonical_count as f64 / total as f64
            } else {
                0.5
            };

            let canon_entry = ArtistCanonicalization {
                id: None,
                canonical_name: canonical.clone(),
                variant_name: variant.clone(),
                confidence: Some(confidence),
                auto_detected: true,
                confirmed_at: None,
            };

            db.upsert_artist_canonicalization(&canon_entry)?;
            new_entries += 1;

            crate::config::log_message(&format!(
                "Canonicalization: '{}' ({} occurrences) -> '{}' ({} occurrences), confidence: {:.2}",
                variant, count, canonical, canonical_count, confidence
            ))?;
        }
    }

    Ok(new_entries)
}

/// Normalize a string for comparison.
/// Converts to lowercase and trims whitespace.
fn normalize_for_comparison(s: &str) -> String {
    s.to_lowercase().trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize() {
        assert_eq!(normalize_for_comparison("  Artist Name  "), "artist name");
        assert_eq!(normalize_for_comparison("ARTIST"), "artist");
    }
}
