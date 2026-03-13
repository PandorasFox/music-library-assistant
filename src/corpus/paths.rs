//! Path Resolution Layer
//!
//! Handles conversion between absolute filesystem paths and root-relative database paths.
//! All paths in the database are stored relative to the single archive root,
//! with their first component identifying the domain:
//!
//! - `corpus/Artist/Album/track.flac` — corpus file
//! - `libraries/music/Artist/Album/track.mp3` — deployed library file
//! - `libraries/legacy/old/track.mp3` — legacy library file
//! - `stash/action_name/Artist/Album/track.flac` — stashed corpus file (preserves structure)
//! - `stash/action_name/music/Artist/Album/track.mp3` — stashed library file (preserves structure)
//!
//! This self-classifying path scheme means no external routing is needed —
//! the path's first component IS the type tag.

use std::sync::OnceLock;

use crate::config;

// Re-export PathResolver and free functions from mm-meta
pub use mm_meta::paths::{
    is_corpus_path, is_library_path, read_mtime, PathResolver,
};

// ============================================================================
// mm-specific globals (not in mm-meta)
// ============================================================================

/// Global PathResolver for use throughout the codebase.
/// Initialized lazily on first use from config.
static GLOBAL_RESOLVER: OnceLock<PathResolver> = OnceLock::new();

/// Expected filesystem device ID (st_dev) for all paths in the archive.
///
/// Set during config validation after checking that root, corpus, libraries,
/// and stash are all on the same filesystem. Used at runtime to detect if
/// any path crosses a mount boundary (nested mount point).
static EXPECTED_DEVICE_ID: OnceLock<u64> = OnceLock::new();

/// Get the expected device ID for filesystem boundary checks.
///
/// Returns None if not yet set. Currently always None — filesystem validation
/// needs reimplementation as a mutation-triggered check that populates
/// EXPECTED_DEVICE_ID via OnceLock::set().
pub fn get_expected_device_id() -> Option<u64> {
    EXPECTED_DEVICE_ID.get().copied()
}

/// Get the global PathResolver, initializing from config if needed.
///
/// This is a convenience function for code that doesn't have direct access
/// to the Witch's PathResolver (e.g., worker threads, computations).
pub fn get_resolver() -> &'static PathResolver {
    GLOBAL_RESOLVER.get_or_init(|| {
        let cfg = config::load_config().expect("Failed to load config for path resolution");
        PathResolver::from_config(&cfg)
    })
}

/// Convert an absolute path to a root-relative path, returning an error if
/// the path is not within the archive root.
///
/// This is a convenience wrapper around `get_resolver().to_relative()` for the
/// common pattern where failure to resolve means an error, not a skip.
pub fn resolve_relative(path: &std::path::Path) -> anyhow::Result<std::path::PathBuf> {
    let resolver = get_resolver();
    resolver.to_relative(path).ok_or_else(|| {
        anyhow::anyhow!(
            "Path {} does not match root. Check config.kdl roots.",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use mm_meta::paths::is_stash_path;
    use mm_utils::t;
    use std::path::{Path, PathBuf};

    fn test_config() -> Config {
        Config {
            root: PathBuf::from("/archive"),
            legacy_enabled: true,
            source_dirs: vec![],
            opinions: Default::default(),
        }
    }

    #[test]
    fn test_to_relative() {
        let resolver = PathResolver::from_config(&test_config());

        let abs = Path::new("/archive/corpus/Artist/Album/track.mp3");
        let rel = t!(resolver.to_relative(abs));
        assert_eq!(rel, PathBuf::from("corpus/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_to_relative_mismatch() {
        let resolver = PathResolver::from_config(&test_config());

        let abs = Path::new("/other/path/track.mp3");
        assert!(resolver.to_relative(abs).is_none());
    }

    #[test]
    fn test_resolve() {
        let resolver = PathResolver::from_config(&test_config());

        let rel = Path::new("corpus/Artist/Album/track.mp3");
        let abs = resolver.resolve(rel);
        assert_eq!(abs, PathBuf::from("/archive/corpus/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_resolve_library() {
        let resolver = PathResolver::from_config(&test_config());

        let rel = Path::new("libraries/music/Artist/Album/track.mp3");
        let abs = resolver.resolve(rel);
        assert_eq!(
            abs,
            PathBuf::from("/archive/libraries/music/Artist/Album/track.mp3")
        );
    }

    #[test]
    fn test_roundtrip() {
        let resolver = PathResolver::from_config(&test_config());

        let original = PathBuf::from("/archive/corpus/Artist/Album/track.flac");
        let rel = t!(resolver.to_relative(&original));
        let back = resolver.resolve(&rel);
        assert_eq!(back, original);
    }

    #[test]
    fn test_classification() {
        assert!(is_corpus_path(Path::new("corpus/Artist/Album/track.flac")));
        assert!(!is_corpus_path(Path::new("libraries/music/track.mp3")));

        assert!(is_library_path(Path::new("libraries/music/track.mp3")));
        assert!(is_library_path(Path::new("libraries/legacy/old/track.mp3")));
        assert!(!is_library_path(Path::new("corpus/track.flac")));

        assert!(is_stash_path(Path::new("stash/cleanup/track.flac")));
        assert!(!is_stash_path(Path::new("corpus/track.flac")));
    }

    #[test]
    fn test_derived_dirs() {
        let resolver = PathResolver::from_config(&test_config());

        assert_eq!(resolver.root(), Path::new("/archive"));
        assert_eq!(resolver.corpus_dir(), PathBuf::from("/archive/corpus"));
        assert_eq!(
            resolver.libraries_dir(),
            PathBuf::from("/archive/libraries")
        );
        assert_eq!(resolver.stash_dir(), PathBuf::from("/archive/stash"));
    }
}
