//! Artist name normalization for collision detection.
//!
//! Normalizes artist and album_artist names by:
//! - Converting to lowercase
//! - Trimming whitespace
//! - Collapsing multiple spaces
//! - Converting underscores to spaces

/// Normalize an artist name for collision detection.
///
/// - Lowercase
/// - Trim whitespace
/// - Collapse multiple spaces
/// - Normalize underscores to spaces
///
/// # Example
///
/// ```
/// use mla_utils::metadata_magic::normalize_artist;
///
/// assert_eq!(normalize_artist("nervous_testpilot"), "nervous testpilot");
/// assert_eq!(normalize_artist("Nervous Testpilot"), "nervous testpilot");
/// ```
pub fn normalize_artist(s: &str) -> String {
    normalize_name_tag(s)
}

/// Normalize an album_artist name (same rules as artist).
pub fn normalize_album_artist(s: &str) -> String {
    normalize_name_tag(s)
}

/// Shared normalization for name-like tags (artist, album_artist).
fn normalize_name_tag(s: &str) -> String {
    s.to_lowercase()
        .replace('_', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
    fn test_normalize_album_artist() {
        assert_eq!(
            normalize_album_artist("Various_Artists"),
            "various artists"
        );
        assert_eq!(
            normalize_album_artist("  VARIOUS  ARTISTS  "),
            "various artists"
        );
    }
}
