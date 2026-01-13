//! Bespoke tag value normalization for collision detection.
//!
//! Re-exports from mla-utils::metadata_magic for backward compatibility.
//! New code should use `mla_utils::metadata_magic` directly.
//!
//! Different tag types have different normalization rules:
//! - Artist/AlbumArtist: Primarily casing and whitespace
//! - Album: EP/LP suffix handling, edition preservation
//! - Genre: Symbol substitution ("and" / "&" / "'n'"), common spelling variants

pub use mla_utils::metadata_magic::{normalize_album_artist, normalize_artist, normalize_genre};

use super::album_normalization;

/// Normalize an album name for collision detection.
///
/// - Uses album_normalization to strip EP/LP suffixes
/// - Preserves edition information (Deluxe, Complete, etc.)
/// - Returns lowercase base_name for comparison
///
/// Example: "My Album EP" and "My Album (LP)" -> same normalized key
pub fn normalize_album(s: &str) -> String {
    let normalized = album_normalization::normalize_album(s);
    // Return lowercase base name with edition if present
    let base = normalized.base_name.to_lowercase();
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
    fn test_normalize_album() {
        assert_eq!(normalize_album("My Album EP"), "my album");
        assert_eq!(normalize_album("My Album (LP)"), "my album");
        assert_eq!(normalize_album("My Album - EP"), "my album");
        assert_eq!(
            normalize_album("My Album Deluxe Edition"),
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
