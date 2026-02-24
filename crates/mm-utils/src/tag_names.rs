//! Tag name matching and normalization.
//!
//! Vorbis comments allow ASCII 0x20-0x7D for field names, meaning separators
//! like underscore, hyphen, and space are all valid. Different tools use
//! different conventions (CATALOGNUMBER vs catalog_number vs Catalog Number).
//!
//! This module provides:
//! - `normalize_tag_name`: Strips separators for canonical comparison
//! - `compound_tag_name_variants`: Generate separator variants for compound tag names
//! - `compound_tag_sql_in`: SQL IN clause fragment for compound tag name matching
//! - `levenshtein_distance`: Edit distance for detecting spelling variants
//! - `tag_names_match`: Fuzzy matching that handles separators and regional variants
//! - `find_tag_value`: Look up a tag value with fuzzy name matching

/// Separator characters that may appear in tag names.
/// These are stripped for normalized comparison.
const SEPARATORS: &[char] = &['_', '-', ' ', '.'];

/// Normalize a tag name by:
/// - Converting to uppercase
/// - Stripping all separator characters
///
/// This gives a canonical form for comparison regardless of separator style.
///
/// # Example
///
/// ```
/// use mm_utils::tag_names::normalize_tag_name;
///
/// assert_eq!(normalize_tag_name("CATALOGNUMBER"), "CATALOGNUMBER");
/// assert_eq!(normalize_tag_name("catalog_number"), "CATALOGNUMBER");
/// assert_eq!(normalize_tag_name("Catalog-Number"), "CATALOGNUMBER");
/// assert_eq!(normalize_tag_name("catalog number"), "CATALOGNUMBER");
/// ```
pub fn normalize_tag_name(name: &str) -> String {
    name.to_uppercase()
        .chars()
        .filter(|c| !SEPARATORS.contains(c))
        .collect()
}

/// Generate compound tag name variants for a two-word tag concept.
///
/// Given two component words (e.g., "ALBUM" and "ARTIST"), returns the combined
/// forms with different separators, in priority order: no separator, underscore, space.
///
/// These are the three valid separator styles for Vorbis comment field names.
/// The no-separator form is the Xiph/Vorbis standard convention.
///
/// # Example
///
/// ```
/// use mm_utils::tag_names::compound_tag_name_variants;
///
/// assert_eq!(
///     compound_tag_name_variants("ALBUM", "ARTIST"),
///     vec!["ALBUMARTIST", "ALBUM_ARTIST", "ALBUM ARTIST"]
/// );
/// ```
// TODO: replace this with a proper system interface for tag name resolution,
// so that canonical tag names and their known variants are defined in one place
// rather than scattered across SQL queries and match arms.
pub fn compound_tag_name_variants(word1: &str, word2: &str) -> Vec<String> {
    vec![
        format!("{}{}", word1, word2),
        format!("{}_{}", word1, word2),
        format!("{} {}", word1, word2),
    ]
}

/// Format compound tag name variants as a SQL `IN (...)` clause fragment.
///
/// Returns a string like `('ALBUMARTIST', 'ALBUM_ARTIST', 'ALBUM ARTIST')` suitable
/// for embedding in SQL WHERE/ON conditions via `format!()`.
///
/// Only safe for compile-time-known word pairs — never use with user input.
pub fn compound_tag_sql_in(word1: &str, word2: &str) -> String {
    let variants = compound_tag_name_variants(word1, word2);
    let quoted: Vec<String> = variants.iter().map(|v| format!("'{}'", v)).collect();
    format!("({})", quoted.join(", "))
}

/// Compute Levenshtein edit distance between two strings.
///
/// Returns the minimum number of single-character edits (insertions, deletions,
/// or substitutions) required to transform `a` into `b`.
///
/// Uses the standard dynamic programming approach with O(min(m,n)) space.
pub fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let m = a_chars.len();
    let n = b_chars.len();

    // Optimize for empty strings
    if m == 0 {
        return n;
    }
    if n == 0 {
        return m;
    }

    // Use two rows instead of full matrix for O(min(m,n)) space
    let mut prev: Vec<usize> = (0..=n).collect();
    let mut curr: Vec<usize> = vec![0; n + 1];

    for i in 1..=m {
        curr[0] = i;
        for j in 1..=n {
            let cost = if a_chars[i - 1] == b_chars[j - 1] { 0 } else { 1 };
            curr[j] = (prev[j] + 1)              // deletion
                .min(curr[j - 1] + 1)            // insertion
                .min(prev[j - 1] + cost);        // substitution
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[n]
}

/// Result of tag name matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagNameMatch {
    /// Exact match (same normalized form)
    Exact,
    /// Likely regional spelling variant (1-2 edits after normalization)
    LikelyVariant,
    /// No match
    NoMatch,
}

/// Check if two tag names match, handling separators and spelling variants.
///
/// Returns:
/// - `Exact` if normalized forms are identical (separator-only differences)
/// - `LikelyVariant` if normalized forms have 1-2 edit distance (e.g., catalog vs catalogue)
/// - `NoMatch` otherwise
///
/// # Example
///
/// ```
/// use mm_utils::tag_names::{tag_names_match, TagNameMatch};
///
/// // Separator differences → Exact
/// assert_eq!(tag_names_match("catalog_number", "CATALOGNUMBER"), TagNameMatch::Exact);
/// assert_eq!(tag_names_match("album_artist", "AlbumArtist"), TagNameMatch::Exact);
///
/// // Regional spelling → LikelyVariant
/// assert_eq!(tag_names_match("catalognumber", "cataloguenumber"), TagNameMatch::LikelyVariant);
///
/// // Too different → NoMatch
/// assert_eq!(tag_names_match("artist", "album"), TagNameMatch::NoMatch);
/// ```
pub fn tag_names_match(name_a: &str, name_b: &str) -> TagNameMatch {
    let norm_a = normalize_tag_name(name_a);
    let norm_b = normalize_tag_name(name_b);

    if norm_a == norm_b {
        return TagNameMatch::Exact;
    }

    // Check for regional spelling variants (1-2 edit distance)
    let distance = levenshtein_distance(&norm_a, &norm_b);
    if distance <= 2 {
        TagNameMatch::LikelyVariant
    } else {
        TagNameMatch::NoMatch
    }
}

/// Result of a tag value lookup, including match quality information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagLookupResult<'a> {
    /// The found value
    pub value: &'a str,
    /// How the tag name matched
    pub match_type: TagNameMatch,
    /// The actual tag name that was found (for logging/diagnostics)
    pub found_name: &'a str,
}

/// Find a tag value by name, using fuzzy matching for the tag name.
///
/// Searches through the provided tags and returns the first value where the
/// tag name matches (either exactly or as a likely variant).
///
/// Returns a `TagLookupResult` that includes match quality information,
/// allowing callers to log warnings for variant matches if desired.
///
/// # Arguments
///
/// * `tags` - Iterator of (tag_name, tag_value) pairs
/// * `target_name` - The tag name to search for
///
/// # Example
///
/// ```ignore
/// let tags = vec![
///     ("catalog_number", "MCB009"),
///     ("artist", "Test"),
/// ];
///
/// // Finds "catalog_number" when looking for "catalognumber"
/// if let Some(result) = find_tag_value(tags.iter().copied(), "catalognumber") {
///     assert_eq!(result.value, "MCB009");
/// }
/// ```
pub fn find_tag_value<'a>(
    tags: impl Iterator<Item = (&'a str, &'a str)>,
    target_name: &str,
) -> Option<TagLookupResult<'a>> {
    let mut exact_match: Option<(&str, &str)> = None;
    let mut variant_match: Option<(&str, &str)> = None;

    for (name, value) in tags {
        match tag_names_match(name, target_name) {
            TagNameMatch::Exact => {
                // When multiple keys normalize to the same form (e.g., ALBUMARTIST
                // and ALBUM_ARTIST), pick deterministically: alphabetical by key,
                // then by value. This prevents HashMap iteration order from producing
                // non-deterministic deploy paths.
                exact_match = Some(match exact_match {
                    None => (name, value),
                    Some((prev_name, prev_value)) => {
                        if (name, value) < (prev_name, prev_value) {
                            (name, value)
                        } else {
                            (prev_name, prev_value)
                        }
                    }
                });
            }
            TagNameMatch::LikelyVariant => {
                // Same tiebreaker for variant matches.
                variant_match = Some(match variant_match {
                    None => (name, value),
                    Some((prev_name, prev_value)) => {
                        if (name, value) < (prev_name, prev_value) {
                            (name, value)
                        } else {
                            (prev_name, prev_value)
                        }
                    }
                });
            }
            TagNameMatch::NoMatch => {}
        }
    }

    if let Some((found_name, value)) = exact_match {
        Some(TagLookupResult {
            value,
            match_type: TagNameMatch::Exact,
            found_name,
        })
    } else {
        variant_match.map(|(found_name, value)| TagLookupResult {
            value,
            match_type: TagNameMatch::LikelyVariant,
            found_name,
        })
    }
}

/// Find a tag value in a HashMap-like structure, using fuzzy matching.
///
/// This is a convenience wrapper for use with `HashMap<String, String>`.
/// Returns just the value string for simple use cases.
pub fn find_tag_in_map<'a>(
    tag_map: &'a std::collections::HashMap<String, String>,
    target_name: &str,
) -> Option<&'a str> {
    find_tag_value(
        tag_map.iter().map(|(k, v)| (k.as_str(), v.as_str())),
        target_name,
    )
    .map(|result| result.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_tag_name() {
        // All should normalize to "CATALOGNUMBER"
        assert_eq!(normalize_tag_name("CATALOGNUMBER"), "CATALOGNUMBER");
        assert_eq!(normalize_tag_name("catalog_number"), "CATALOGNUMBER");
        assert_eq!(normalize_tag_name("Catalog-Number"), "CATALOGNUMBER");
        assert_eq!(normalize_tag_name("catalog number"), "CATALOGNUMBER");
        assert_eq!(normalize_tag_name("Catalog.Number"), "CATALOGNUMBER");
        assert_eq!(normalize_tag_name("CATALOG_NUMBER"), "CATALOGNUMBER");

        // Album artist variations
        assert_eq!(normalize_tag_name("album_artist"), "ALBUMARTIST");
        assert_eq!(normalize_tag_name("ALBUMARTIST"), "ALBUMARTIST");
        assert_eq!(normalize_tag_name("Album Artist"), "ALBUMARTIST");
    }

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("abc", ""), 3);
        assert_eq!(levenshtein_distance("", "xyz"), 3);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
        assert_eq!(levenshtein_distance("abc", "abd"), 1);
        assert_eq!(levenshtein_distance("abc", "abcd"), 1);
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);

        // Regional spelling: catalog vs catalogue
        assert_eq!(levenshtein_distance("catalognumber", "cataloguenumber"), 2);
    }

    #[test]
    fn test_tag_names_match_exact() {
        // Separator differences only
        assert_eq!(
            tag_names_match("catalog_number", "CATALOGNUMBER"),
            TagNameMatch::Exact
        );
        assert_eq!(
            tag_names_match("album_artist", "AlbumArtist"),
            TagNameMatch::Exact
        );
        assert_eq!(
            tag_names_match("track-number", "TRACK_NUMBER"),
            TagNameMatch::Exact
        );
    }

    #[test]
    fn test_tag_names_match_variant() {
        // Regional spelling (UK vs US)
        assert_eq!(
            tag_names_match("catalognumber", "cataloguenumber"),
            TagNameMatch::LikelyVariant
        );
        // "catalogue" has 1 extra char vs "catalog"
        assert_eq!(
            tag_names_match("catalog_number", "CATALOGUENUMBER"),
            TagNameMatch::LikelyVariant
        );
    }

    #[test]
    fn test_tag_names_match_no_match() {
        assert_eq!(tag_names_match("artist", "album"), TagNameMatch::NoMatch);
        assert_eq!(tag_names_match("genre", "title"), TagNameMatch::NoMatch);
        assert_eq!(
            tag_names_match("catalognumber", "something_else"),
            TagNameMatch::NoMatch
        );
    }

    #[test]
    fn test_find_tag_value_exact() {
        let tags = vec![
            ("catalog_number", "MCB009"),
            ("artist", "Test Artist"),
            ("album", "Test Album"),
        ];

        // Exact match (after normalization)
        let result = find_tag_value(tags.iter().copied(), "catalognumber");
        assert!(result.is_some());
        let result = result.unwrap();
        assert_eq!(result.value, "MCB009");
        assert_eq!(result.match_type, TagNameMatch::Exact);

        let result2 = find_tag_value(tags.iter().copied(), "CATALOG_NUMBER");
        assert!(result2.is_some());
        assert_eq!(result2.unwrap().value, "MCB009");
    }

    #[test]
    fn test_find_tag_value_not_found() {
        let tags = vec![("artist", "Test"), ("album", "Album")];
        assert!(find_tag_value(tags.iter().copied(), "catalognumber").is_none());
    }

    #[test]
    fn test_find_tag_in_map() {
        use std::collections::HashMap;

        let mut tag_map = HashMap::new();
        tag_map.insert("catalog_number".to_string(), "MCB009".to_string());
        tag_map.insert("artist".to_string(), "Test".to_string());

        assert_eq!(find_tag_in_map(&tag_map, "catalognumber"), Some("MCB009"));
        assert_eq!(find_tag_in_map(&tag_map, "CATALOG_NUMBER"), Some("MCB009"));
        assert_eq!(find_tag_in_map(&tag_map, "genre"), None);
    }

    #[test]
    fn test_find_tag_value_deterministic_tiebreak() {
        use std::collections::HashMap;

        // Both ALBUMARTIST and ALBUM_ARTIST normalize to the same form.
        // When they have different values, the result must be deterministic
        // regardless of HashMap iteration order.
        let mut tag_map = HashMap::new();
        tag_map.insert("ALBUMARTIST".to_string(), "RAWRDCORE RECORDS".to_string());
        tag_map.insert("ALBUM_ARTIST".to_string(), "4lung".to_string());

        // "ALBUMARTIST" < "ALBUM_ARTIST" in ASCII (A=0x41 < _=0x5F at position 5)
        // So ALBUMARTIST wins the tiebreak.
        let result = find_tag_in_map(&tag_map, "albumartist");
        assert_eq!(result, Some("RAWRDCORE RECORDS"));

        // Verify stability across many calls (HashMap iteration order can vary)
        for _ in 0..100 {
            assert_eq!(find_tag_in_map(&tag_map, "albumartist"), Some("RAWRDCORE RECORDS"));
        }
    }

    #[test]
    fn test_find_tag_value_tiebreak_by_value() {
        // When keys are identical after normalization and one is an exact string match,
        // it should still be deterministic. If keys are the same string, tiebreak by value.
        let tags = vec![
            ("ARTIST", "Zebra"),
            ("ARTIST", "Alpha"),
        ];

        // Same key name: tiebreak by value, "Alpha" < "Zebra"
        let result = find_tag_value(tags.iter().copied(), "artist");
        assert!(result.is_some());
        assert_eq!(result.unwrap().value, "Alpha");
    }
}
