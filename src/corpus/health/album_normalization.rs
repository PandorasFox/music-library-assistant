//! Album name normalization for fuzzy matching.
//!
//! Re-exports from mm-utils::metadata_magic::album.
//! New code should use `mm_utils::metadata_magic` directly.

pub use mm_utils::metadata_magic::album::normalize_album;

// Re-export for tests only
#[cfg(test)]
use mm_utils::metadata_magic::album::albums_equivalent;

#[cfg(test)]
mod tests {
    use super::*;
    use mm_utils::metadata_magic::AlbumFormat;

    // Basic smoke tests to verify re-exports work
    #[test]
    fn test_normalize_ep_variants() {
        let norm = normalize_album("My Album EP");
        assert_eq!(norm.base_name, "My Album");
        assert_eq!(norm.format_type, AlbumFormat::EP);
    }

    #[test]
    fn test_albums_equivalent() {
        assert!(albums_equivalent("My Album EP", "My Album (EP)"));
        assert!(!albums_equivalent("My Album Deluxe Edition", "My Album"));
    }
}
