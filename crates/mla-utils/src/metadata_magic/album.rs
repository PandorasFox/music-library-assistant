//! Album name normalization for fuzzy matching.
//!
//! Handles format suffixes (EP, LP) as equivalent while preserving
//! meaningful edition suffixes (Deluxe, Complete, etc.).

use std::borrow::Cow;

/// Album format type (stripped for equivalence matching)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlbumFormat {
    EP,
    LP,
    /// No explicit format suffix
    Standard,
}

/// Result of normalizing an album name
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedAlbum {
    /// Base album name with format suffix stripped
    pub base_name: String,
    /// Detected format type (EP, LP, or Standard)
    pub format_type: AlbumFormat,
    /// Edition suffix if present (e.g., "Deluxe Edition")
    pub edition: Option<String>,
    /// Original album name for reference
    pub original: String,
}

/// Format suffix patterns to strip for normalization.
/// These make albums equivalent: "Album EP" == "Album (EP)" == "Album - EP"
/// IMPORTANT: Longer patterns must come first so they match before shorter ones.
const FORMAT_SUFFIXES: &[(&str, AlbumFormat)] = &[
    // EP variants (longest first)
    (" - EP", AlbumFormat::EP),
    (" (EP)", AlbumFormat::EP),
    (" [EP]", AlbumFormat::EP),
    (" EP", AlbumFormat::EP),
    // LP variants (longest first)
    (" - LP", AlbumFormat::LP),
    (" (LP)", AlbumFormat::LP),
    (" [LP]", AlbumFormat::LP),
    (" LP", AlbumFormat::LP),
];

/// Edition keywords that PRESERVE distinction between albums.
/// These are extracted but kept as part of identity.
const EDITION_PATTERNS: &[&str] = &[
    "Deluxe Edition",
    "Deluxe",
    "Complete Edition",
    "Complete",
    "Standard Edition",
    "Standard",
    "Special Edition",
    "Special",
    "Expanded Edition",
    "Expanded",
    "Anniversary Edition",
    "Anniversary",
    "Collector's Edition",
    "Collector",
    "Limited Edition",
    "Limited",
    "Remastered Edition",
    "Remastered",
    "Remaster",
];

/// Normalize an album name for comparison.
///
/// Strips format suffixes (EP, LP) while preserving edition suffixes.
///
/// # Examples
///
/// ```
/// use mla_utils::metadata_magic::{normalize_album, AlbumFormat};
///
/// let norm = normalize_album("My Album EP");
/// assert_eq!(norm.base_name, "My Album");
/// assert_eq!(norm.format_type, AlbumFormat::EP);
/// assert_eq!(norm.edition, None);
///
/// let norm = normalize_album("My Album (Deluxe Edition)");
/// assert_eq!(norm.base_name, "My Album");
/// assert_eq!(norm.edition, Some("Deluxe Edition".to_string()));
/// ```
pub fn normalize_album(album: &str) -> NormalizedAlbum {
    let trimmed = album.trim();
    let mut working = Cow::Borrowed(trimmed);
    let mut format_type = AlbumFormat::Standard;
    let mut edition: Option<String> = None;

    // First, extract edition suffix (case-insensitive search, preserve original casing)
    let lower = working.to_lowercase();
    for pattern in EDITION_PATTERNS {
        let pattern_lower = pattern.to_lowercase();

        // Look for pattern in parentheses: "(Deluxe Edition)"
        if let Some(paren_start) = lower.find(&format!("({}", pattern_lower)) {
            // Find the closing paren
            if let Some(rel_end) = lower[paren_start..].find(')') {
                let paren_end = paren_start + rel_end + 1;
                // Extract the edition text (without parens)
                let edition_text = trimmed[paren_start + 1..paren_end - 1].trim().to_string();
                edition = Some(edition_text);
                // Remove the parenthesized edition from working string
                let before = working[..paren_start].trim_end();
                let after = working.get(paren_end..).map(|s| s.trim_start()).unwrap_or("");
                let new_working = if !before.is_empty() && !after.is_empty() {
                    format!("{} {}", before, after)
                } else if !before.is_empty() {
                    before.to_string()
                } else {
                    after.to_string()
                };
                working = Cow::Owned(new_working.trim().to_string());
                break;
            }
        }

        // Look for pattern as suffix: "Album Deluxe Edition"
        if lower.ends_with(&pattern_lower) {
            let suffix_start = working.len() - pattern.len();
            // Make sure it's a word boundary (space or start)
            if suffix_start == 0 || working.as_bytes().get(suffix_start - 1) == Some(&b' ') {
                let edition_text = trimmed[suffix_start..].trim().to_string();
                edition = Some(edition_text);
                working = Cow::Owned(working[..suffix_start].trim().to_string());
                break;
            }
        }
    }

    // Then, strip format suffixes
    let lower = working.to_lowercase();

    // Handle standalone format strings first (e.g., just "EP" or "LP")
    if lower == "ep" {
        format_type = AlbumFormat::EP;
        working = Cow::Owned(String::new());
    } else if lower == "lp" {
        format_type = AlbumFormat::LP;
        working = Cow::Owned(String::new());
    } else {
        // Check format suffix patterns
        for (suffix, fmt) in FORMAT_SUFFIXES {
            let suffix_lower = suffix.to_lowercase();
            if lower.ends_with(&suffix_lower) {
                format_type = *fmt;
                let new_len = working.len() - suffix.len();
                working = Cow::Owned(working[..new_len].trim().to_string());
                break;
            }
        }
    }

    NormalizedAlbum {
        base_name: working.into_owned(),
        format_type,
        edition,
        original: album.to_string(),
    }
}

/// Check if two album names are equivalent (ignoring format suffixes).
///
/// Two albums are equivalent if they have:
/// - Same base name (case-insensitive)
/// - Same edition (or both no edition)
///
/// Format suffixes (EP, LP) are ignored.
///
/// # Examples
///
/// ```
/// use mla_utils::metadata_magic::albums_equivalent;
///
/// assert!(albums_equivalent("My Album EP", "My Album (EP)"));
/// assert!(albums_equivalent("My Album LP", "My Album - LP"));
/// assert!(!albums_equivalent("My Album Deluxe Edition", "My Album"));
/// ```
pub fn albums_equivalent(a: &str, b: &str) -> bool {
    let norm_a = normalize_album(a);
    let norm_b = normalize_album(b);

    // Compare base names case-insensitively
    let base_match = norm_a.base_name.to_lowercase() == norm_b.base_name.to_lowercase();

    // Compare editions (both None, or both Some with same value case-insensitively)
    let edition_match = match (&norm_a.edition, &norm_b.edition) {
        (None, None) => true,
        (Some(e1), Some(e2)) => e1.to_lowercase() == e2.to_lowercase(),
        _ => false,
    };

    base_match && edition_match
}

/// Check if album `a` is a superior edition of album `b`.
///
/// Returns true if `a` has a "better" edition suffix than `b`.
/// Used for detecting when standard editions can be replaced by deluxe/complete editions.
pub fn is_superior_edition(a: &str, b: &str) -> bool {
    let norm_a = normalize_album(a);
    let norm_b = normalize_album(b);

    // Must have same base name
    if norm_a.base_name.to_lowercase() != norm_b.base_name.to_lowercase() {
        return false;
    }

    let rank_a = edition_rank(norm_a.edition.as_deref());
    let rank_b = edition_rank(norm_b.edition.as_deref());

    rank_a > rank_b
}

/// Get the rank of an edition (higher = more desirable).
///
/// - 0: No edition or "Standard Edition"
/// - 1: Special, Expanded, Limited
/// - 2: Deluxe, Anniversary, Remastered
/// - 3: Complete, Collector's
pub fn edition_rank(edition: Option<&str>) -> u8 {
    match edition.map(|s| s.to_lowercase()).as_deref() {
        None => 0,
        Some(s) if s.contains("standard") => 0,
        Some(s) if s.contains("complete") => 3,
        Some(s) if s.contains("collector") => 3,
        Some(s) if s.contains("deluxe") => 2,
        Some(s) if s.contains("anniversary") => 2,
        Some(s) if s.contains("remaster") => 2,
        Some(s) if s.contains("special") => 1,
        Some(s) if s.contains("expanded") => 1,
        Some(s) if s.contains("limited") => 1,
        Some(_) => 1, // Unknown edition gets rank 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_ep_variants() {
        let cases = [
            ("My Album EP", "My Album", AlbumFormat::EP),
            ("My Album (EP)", "My Album", AlbumFormat::EP),
            ("My Album - EP", "My Album", AlbumFormat::EP),
            ("My Album [EP]", "My Album", AlbumFormat::EP),
        ];

        for (input, expected_base, expected_format) in cases {
            let norm = normalize_album(input);
            assert_eq!(
                norm.base_name, expected_base,
                "Failed for input: {}",
                input
            );
            assert_eq!(
                norm.format_type, expected_format,
                "Failed for input: {}",
                input
            );
            assert_eq!(norm.edition, None, "Failed for input: {}", input);
        }
    }

    #[test]
    fn test_normalize_lp_variants() {
        let cases = [
            ("My Album LP", "My Album", AlbumFormat::LP),
            ("My Album (LP)", "My Album", AlbumFormat::LP),
            ("My Album - LP", "My Album", AlbumFormat::LP),
            ("My Album [LP]", "My Album", AlbumFormat::LP),
        ];

        for (input, expected_base, expected_format) in cases {
            let norm = normalize_album(input);
            assert_eq!(
                norm.base_name, expected_base,
                "Failed for input: {}",
                input
            );
            assert_eq!(
                norm.format_type, expected_format,
                "Failed for input: {}",
                input
            );
        }
    }

    #[test]
    fn test_preserve_edition_suffixes() {
        let norm = normalize_album("My Album (Deluxe Edition)");
        assert_eq!(norm.base_name, "My Album");
        assert_eq!(norm.edition, Some("Deluxe Edition".to_string()));
        assert_eq!(norm.format_type, AlbumFormat::Standard);

        let norm = normalize_album("My Album Deluxe Edition");
        assert_eq!(norm.base_name, "My Album");
        assert_eq!(norm.edition, Some("Deluxe Edition".to_string()));

        let norm = normalize_album("My Album (Complete Edition)");
        assert_eq!(norm.base_name, "My Album");
        assert_eq!(norm.edition, Some("Complete Edition".to_string()));

        let norm = normalize_album("My Album Remastered");
        assert_eq!(norm.base_name, "My Album");
        assert_eq!(norm.edition, Some("Remastered".to_string()));
    }

    #[test]
    fn test_edition_with_format() {
        // Edition + format: "Album Deluxe Edition EP"
        let norm = normalize_album("My Album (Deluxe Edition) EP");
        assert_eq!(norm.base_name, "My Album");
        assert_eq!(norm.edition, Some("Deluxe Edition".to_string()));
        assert_eq!(norm.format_type, AlbumFormat::EP);
    }

    #[test]
    fn test_albums_equivalent() {
        // Same base, different format suffixes - should be equivalent
        assert!(albums_equivalent("My Album EP", "My Album (EP)"));
        assert!(albums_equivalent("My Album EP", "My Album - EP"));
        assert!(albums_equivalent("My Album LP", "My Album (LP)"));
        assert!(albums_equivalent("My Album EP", "My Album LP")); // Format doesn't matter
        assert!(albums_equivalent("My Album", "My Album EP")); // No format vs format

        // Different editions - NOT equivalent
        assert!(!albums_equivalent(
            "My Album Deluxe Edition",
            "My Album Standard Edition"
        ));
        assert!(!albums_equivalent("My Album Deluxe Edition", "My Album"));
        assert!(!albums_equivalent(
            "My Album (Complete Edition)",
            "My Album"
        ));

        // Same edition - equivalent
        assert!(albums_equivalent(
            "My Album (Deluxe Edition)",
            "My Album Deluxe Edition"
        ));
        assert!(albums_equivalent(
            "My Album (Deluxe Edition) EP",
            "My Album (Deluxe Edition) LP"
        ));

        // Case insensitive
        assert!(albums_equivalent("my album", "MY ALBUM"));
        assert!(albums_equivalent(
            "My Album (deluxe edition)",
            "My Album (DELUXE EDITION)"
        ));
    }

    #[test]
    fn test_edition_rank() {
        assert_eq!(edition_rank(None), 0);
        assert_eq!(edition_rank(Some("Standard Edition")), 0);
        assert_eq!(edition_rank(Some("Special Edition")), 1);
        assert_eq!(edition_rank(Some("Expanded")), 1);
        assert_eq!(edition_rank(Some("Deluxe Edition")), 2);
        assert_eq!(edition_rank(Some("Anniversary Edition")), 2);
        assert_eq!(edition_rank(Some("Remastered")), 2);
        assert_eq!(edition_rank(Some("Complete Edition")), 3);
        assert_eq!(edition_rank(Some("Collector's Edition")), 3);
    }

    #[test]
    fn test_is_superior_edition() {
        assert!(is_superior_edition("My Album Deluxe Edition", "My Album"));
        assert!(is_superior_edition(
            "My Album (Complete Edition)",
            "My Album Deluxe Edition"
        ));
        assert!(is_superior_edition(
            "My Album (Deluxe Edition)",
            "My Album (Standard Edition)"
        ));

        // Not superior (same or lower rank)
        assert!(!is_superior_edition("My Album", "My Album Deluxe Edition"));
        assert!(!is_superior_edition(
            "My Album Deluxe Edition",
            "My Album Deluxe Edition"
        ));

        // Different base albums - never superior
        assert!(!is_superior_edition(
            "Other Album Deluxe Edition",
            "My Album"
        ));
    }

    #[test]
    fn test_edge_cases() {
        // Album name that starts with "EP" - should not strip
        let norm = normalize_album("EP-1");
        assert_eq!(norm.base_name, "EP-1");
        assert_eq!(norm.format_type, AlbumFormat::Standard);

        // Empty string
        let norm = normalize_album("");
        assert_eq!(norm.base_name, "");

        // Only format suffix
        let norm = normalize_album("EP");
        assert_eq!(norm.base_name, "");
        assert_eq!(norm.format_type, AlbumFormat::EP);

        // Whitespace handling
        let norm = normalize_album("  My Album  EP  ");
        assert_eq!(norm.base_name, "My Album");
        assert_eq!(norm.format_type, AlbumFormat::EP);
    }
}
