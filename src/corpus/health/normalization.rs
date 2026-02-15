//! Bespoke tag value normalization for collision detection.
//!
//! Re-exports from mm-utils::metadata_magic.
//! New code should use `mm_utils::metadata_magic` directly.
//!
//! Different tag types have different normalization rules:
//! - Artist/AlbumArtist: Primarily casing and whitespace
//! - Album: EP/LP suffix handling, edition preservation
//! - Genre: Symbol substitution ("and" / "&" / "'n'"), common spelling variants

pub use mm_utils::metadata_magic::{normalize_album_artist, normalize_artist, normalize_genre};

use super::album_normalization;

/// Normalize an album name for collision detection.
///
/// - Uses album_normalization to strip EP/LP suffixes
/// - Preserves edition information (Deluxe, Complete, etc.)
/// - Returns lowercase base_name for comparison
///
/// When `strip_format_suffixes` is true (default), EP/LP suffixes are stripped
/// so "My Album EP" and "My Album (LP)" produce the same key.
/// When false, the format type is re-appended so they produce distinct keys.
pub fn normalize_album(s: &str, strip_format_suffixes: bool) -> String {
    use mm_utils::metadata_magic::AlbumFormat;

    let normalized = album_normalization::normalize_album(s);
    let base = normalized.base_name.to_lowercase();
    let base = if !strip_format_suffixes {
        match normalized.format_type {
            AlbumFormat::EP => format!("{} ep", base),
            AlbumFormat::LP => format!("{} lp", base),
            AlbumFormat::Standard => base,
        }
    } else {
        base
    };
    if let Some(edition) = &normalized.edition {
        format!("{} [{}]", base, edition.to_lowercase())
    } else {
        base
    }
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
    fn test_normalize_album_strip_suffixes() {
        // Default behavior: strip EP/LP
        assert_eq!(normalize_album("My Album EP", true), "my album");
        assert_eq!(normalize_album("My Album (LP)", true), "my album");
        assert_eq!(normalize_album("My Album - EP", true), "my album");
        assert_eq!(
            normalize_album("My Album Deluxe Edition", true),
            "my album [deluxe edition]"
        );
    }

    #[test]
    fn test_normalize_album_preserve_suffixes() {
        // When strip_format_suffixes is false, EP/LP produce distinct keys
        assert_eq!(normalize_album("My Album EP", false), "my album ep");
        assert_eq!(normalize_album("My Album (LP)", false), "my album lp");
        assert_eq!(normalize_album("My Album", false), "my album");
        // Standard albums are unchanged
        assert_eq!(
            normalize_album("My Album Deluxe Edition", false),
            "my album [deluxe edition]"
        );
    }

    #[test]
    fn test_normalize_genre() {
        // Hip-hop variants
        assert_eq!(normalize_genre("Hip Hop"), "hip-hop");
        assert_eq!(normalize_genre("Hip-Hop"), "hip-hop");
        assert_eq!(normalize_genre("HipHop"), "hip-hop");

        // R&B variants
        assert_eq!(normalize_genre("R&B"), "r&b");
        assert_eq!(normalize_genre("R & B"), "r&b");
        assert_eq!(normalize_genre("RnB"), "r&b");
        assert_eq!(normalize_genre("Rhythm and Blues"), "r&b");

        // Drum & Bass variants
        assert_eq!(normalize_genre("Drum and Bass"), "drum & bass");
        assert_eq!(normalize_genre("Drum n Bass"), "drum & bass");
        assert_eq!(normalize_genre("DnB"), "drum & bass");

        // Casing and whitespace
        assert_eq!(normalize_genre("  ROCK  "), "rock");
        assert_eq!(normalize_genre("Electronic"), "electronic");
    }

    #[test]
    fn test_normalize_genre_separators() {
        assert_eq!(normalize_genre("Rock and Roll"), "rock & roll");
        assert_eq!(normalize_genre("Rock 'n' Roll"), "rock & roll");
        assert_eq!(normalize_genre("Rock n Roll"), "rock & roll");
    }
}
