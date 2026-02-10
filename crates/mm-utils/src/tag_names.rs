//! Tag name matching and normalization.
//!
//! Vorbis comments allow ASCII 0x20-0x7D for field names, meaning separators
//! like underscore, hyphen, and space are all valid. Different tools use
//! different conventions (CATALOGNUMBER vs catalog_number vs Catalog Number).
//!
//! This module provides:
//! - `normalize_tag_name`: Strips separators for canonical comparison
//! - `levenshtein_distance`: Edit distance for detecting spelling variants
//! - `tag_names_match`: Fuzzy matching that handles separators and regional variants
//! - `find_tag_value`: Look up a tag value with fuzzy name matching

/// Separator characters that may appear in tag names.
/// These are stripped for normalized comparison.
const SEPARATORS: &[char] = &['_', '-', ' ', '.'];

/// Normalize a tag name by:
/// - Converting to lowercase
/// - Stripping all separator characters
///
/// This gives a canonical form for comparison regardless of separator style.
///
/// # Example
///
/// ```
/// use mm_utils::tag_names::normalize_tag_name;
///
/// assert_eq!(normalize_tag_name("CATALOGNUMBER"), "catalognumber");
/// assert_eq!(normalize_tag_name("catalog_number"), "catalognumber");
/// assert_eq!(normalize_tag_name("Catalog-Number"), "catalognumber");
/// assert_eq!(normalize_tag_name("catalog number"), "catalognumber");
/// ```
pub fn normalize_tag_name(name: &str) -> String {
    name.to_lowercase()
        .chars()
        .filter(|c| !SEPARATORS.contains(c))
        .collect()
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
    let mut variant_match: Option<(&str, &str)> = None;

    for (name, value) in tags {
        match tag_names_match(name, target_name) {
            TagNameMatch::Exact => {
                return Some(TagLookupResult {
                    value,
                    match_type: TagNameMatch::Exact,
                    found_name: name,
                });
            }
            TagNameMatch::LikelyVariant => {
                // Store first variant match, but keep looking for exact
                if variant_match.is_none() {
                    variant_match = Some((name, value));
                }
            }
            TagNameMatch::NoMatch => {}
        }
    }

    // Return variant match if found
    variant_match.map(|(found_name, value)| TagLookupResult {
        value,
        match_type: TagNameMatch::LikelyVariant,
        found_name,
    })
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
        // All should normalize to "catalognumber"
        assert_eq!(normalize_tag_name("CATALOGNUMBER"), "catalognumber");
        assert_eq!(normalize_tag_name("catalog_number"), "catalognumber");
        assert_eq!(normalize_tag_name("Catalog-Number"), "catalognumber");
        assert_eq!(normalize_tag_name("catalog number"), "catalognumber");
        assert_eq!(normalize_tag_name("Catalog.Number"), "catalognumber");
        assert_eq!(normalize_tag_name("CATALOG_NUMBER"), "catalognumber");

        // Album artist variations
        assert_eq!(normalize_tag_name("album_artist"), "albumartist");
        assert_eq!(normalize_tag_name("ALBUMARTIST"), "albumartist");
        assert_eq!(normalize_tag_name("Album Artist"), "albumartist");
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
}
