//! Artist name normalization for collision detection.
//!
//! Normalizes artist and album_artist names by:
//! - Converting to lowercase
//! - Trimming whitespace
//! - Collapsing multiple spaces
//! - Converting underscores to spaces
//! - Converting " & " to " and " (band name separator normalization)
//! - Stripping leading "the " article so "The X" and "X" collide

/// Normalize an artist name for collision detection.
///
/// - Lowercase
/// - Trim whitespace
/// - Collapse multiple spaces
/// - Normalize underscores to spaces
/// - Normalize " & " to " and "
/// - Strip leading "the " so "The Black Keys" and "Black Keys" collide
///
/// # Example
///
/// ```
/// use mm_utils::metadata_magic::normalize_artist;
///
/// assert_eq!(normalize_artist("nervous_testpilot"), "nervous testpilot");
/// assert_eq!(normalize_artist("Nervous Testpilot"), "nervous testpilot");
/// assert_eq!(normalize_artist("Toots & the Maytals"), "toots and the maytals");
/// assert_eq!(normalize_artist("Toots and the Maytals"), "toots and the maytals");
/// assert_eq!(normalize_artist("The Black Keys"), "black keys");
/// assert_eq!(normalize_artist("Black Keys"), "black keys");
/// ```
pub fn normalize_artist(s: &str) -> String {
    normalize_name_tag(s)
}

/// Normalize an album_artist name (same rules as artist).
pub fn normalize_album_artist(s: &str) -> String {
    normalize_name_tag(s)
}

/// Convert an artist name to canonical sort form.
///
/// Applies two transforms:
/// - Moves a leading "The " to the end: "The Black Keys" → "Black Keys, The"
/// - Converts " & " to " and ": "Craig G & Marley Marl" → "Craig G and Marley Marl"
///
/// Used to pre-fill the canonical value field when resolving artist tag collisions.
pub fn article_sort_form(s: &str) -> String {
    const ARTICLE: &str = "The ";
    let s = if s.len() > ARTICLE.len() && s[..ARTICLE.len()].eq_ignore_ascii_case(ARTICLE) {
        format!("{}, The", &s[ARTICLE.len()..])
    } else {
        s.to_string()
    };
    s.replace(" & ", " and ")
}

/// Shared normalization for name-like tags (artist, album_artist).
fn normalize_name_tag(s: &str) -> String {
    let normalized = s.to_lowercase()
        .replace('_', " ")
        .replace(" & ", " and ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    normalized.strip_prefix("the ").unwrap_or(&normalized).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_artist() {
        assert_eq!(normalize_artist("nervous_testpilot"), "nervous testpilot");
        assert_eq!(normalize_artist("Nervous Testpilot"), "nervous testpilot");
        assert_eq!(normalize_artist("NERVOUS  TESTPILOT"), "nervous testpilot");
        assert_eq!(
            normalize_artist("  Nervous  Testpilot  "),
            "nervous testpilot"
        );
    }

    #[test]
    fn test_normalize_artist_ampersand() {
        assert_eq!(normalize_artist("Toots & the Maytals"), "toots and the maytals");
        assert_eq!(normalize_artist("Toots and the Maytals"), "toots and the maytals");
        assert_eq!(normalize_artist("Toots & The Maytals"), "toots and the maytals");
        // & without surrounding spaces (e.g. "R&B" in a name) is not converted
        assert_eq!(normalize_artist("Me&You"), "me&you");
    }

    #[test]
    fn test_article_sort_form() {
        assert_eq!(article_sort_form("The Black Keys"), "Black Keys, The");
        assert_eq!(article_sort_form("The Roots"), "Roots, The");
        assert_eq!(article_sort_form("THE NATIONAL"), "NATIONAL, The");
        assert_eq!(article_sort_form("the xx"), "xx, The");
        assert_eq!(article_sort_form("The The"), "The, The");
        // No leading article — unchanged
        assert_eq!(article_sort_form("Tortoise"), "Tortoise");
        assert_eq!(article_sort_form("Thee Oh Sees"), "Thee Oh Sees");
        // "The" alone (no content after) — unchanged
        assert_eq!(article_sort_form("The"), "The");
        // Ampersand conversion
        assert_eq!(article_sort_form("Craig G & Marley Marl"), "Craig G and Marley Marl");
        assert_eq!(article_sort_form("Count Sticky & the Upsetters"), "Count Sticky and the Upsetters");
        // Both transforms together
        assert_eq!(article_sort_form("The Black & White Band"), "Black and White Band, The");
    }

    #[test]
    fn test_normalize_artist_the_prefix() {
        assert_eq!(normalize_artist("The Black Keys"), "black keys");
        assert_eq!(normalize_artist("Black Keys"), "black keys");
        assert_eq!(normalize_artist("The Roots"), "roots");
        // "The The" strips one article, leaving "the"
        assert_eq!(normalize_artist("The The"), "the");
        // "theatre", "then", etc. are not stripped (no trailing space after "the")
        assert_eq!(normalize_artist("Thee Oh Sees"), "thee oh sees");
        assert_eq!(normalize_artist("Theatre of Hate"), "theatre of hate");
    }

    #[test]
    fn test_normalize_album_artist() {
        assert_eq!(normalize_album_artist("Various_Artists"), "various artists");
        assert_eq!(
            normalize_album_artist("  VARIOUS  ARTISTS  "),
            "various artists"
        );
    }
}
