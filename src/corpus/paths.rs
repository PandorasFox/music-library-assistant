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

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::{self, Config};

/// Global PathResolver for use throughout the codebase.
/// Initialized lazily on first use from config.
static GLOBAL_RESOLVER: OnceLock<PathResolver> = OnceLock::new();

/// Expected filesystem device ID (st_dev) for all paths in the archive.
///
/// Set during config validation after checking that root, corpus, libraries,
/// and stash are all on the same filesystem. Used at runtime to detect if
/// any path crosses a mount boundary (nested mount point).
static EXPECTED_DEVICE_ID: OnceLock<u64> = OnceLock::new();

/// Set the expected device ID for filesystem boundary checks.
///
/// Called from config validation after confirming all directories share the same device.
/// Should only be called once at startup.
pub fn set_expected_device_id(dev: u64) {
    let _ = EXPECTED_DEVICE_ID.set(dev);
}

/// Get the expected device ID for filesystem boundary checks.
///
/// Returns None if not yet set (config validation hasn't run).
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
        PathResolver::new(&cfg)
    })
}

/// Resolves paths between absolute filesystem paths and root-relative database paths.
///
/// Created from Config at startup, stored in Witch for access throughout the app.
/// With the single-root model, there's one root and two operations:
/// - `to_relative`: strip root prefix (absolute → root-relative)
/// - `resolve`: join to root (root-relative → absolute)
#[derive(Debug, Clone)]
pub struct PathResolver {
    root: PathBuf,
}

impl PathResolver {
    /// Create a new PathResolver from configuration.
    pub fn new(config: &Config) -> Self {
        Self {
            root: config.root.clone(),
        }
    }

    /// Convert an absolute filesystem path to a root-relative path for database storage.
    ///
    /// Returns None if the path doesn't start with the archive root.
    pub fn to_relative(&self, abs: &Path) -> Option<PathBuf> {
        abs.strip_prefix(&self.root).ok().map(PathBuf::from)
    }

    /// Resolve a root-relative path to an absolute filesystem path.
    pub fn resolve(&self, rel: &Path) -> PathBuf {
        self.root.join(rel)
    }

    /// Get the archive root path.
    #[cfg(test)]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Get the corpus directory path.
    pub fn corpus_dir(&self) -> PathBuf {
        self.root.join("corpus")
    }

    /// Get the libraries directory path.
    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    /// Get the stash directory path.
    #[cfg(test)]
    pub fn stash_dir(&self) -> PathBuf {
        self.root.join("stash")
    }

    /// Get the inbox directory path.
    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }
}

// =============================================================================
// Filesystem Metadata Helpers
// =============================================================================

/// Extract modification time from metadata as (seconds, nanoseconds) tuple.
///
/// Returns (0, 0) if mtime extraction fails. Uses the portable `modified()` API
/// for consistency across all callsites (indexing, observation, tag writes).
pub fn read_mtime(metadata: &std::fs::Metadata) -> (i64, i64) {
    use std::time::UNIX_EPOCH;
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((0, 0))
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

// =============================================================================
// Path Classification (free functions — no resolver needed)
// =============================================================================

/// Check if a root-relative path is a corpus path.
pub fn is_corpus_path(rel: &Path) -> bool {
    rel.starts_with("corpus")
}

/// Check if a root-relative path is a library path.
pub fn is_library_path(rel: &Path) -> bool {
    rel.starts_with("libraries")
}

/// Check if a root-relative path is a stash path.
#[cfg(test)]
pub fn is_stash_path(rel: &Path) -> bool {
    rel.starts_with("stash")
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let resolver = PathResolver::new(&test_config());

        let abs = Path::new("/archive/corpus/Artist/Album/track.mp3");
        let rel = resolver.to_relative(abs).unwrap();
        assert_eq!(rel, PathBuf::from("corpus/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_to_relative_mismatch() {
        let resolver = PathResolver::new(&test_config());

        let abs = Path::new("/other/path/track.mp3");
        assert!(resolver.to_relative(abs).is_none());
    }

    #[test]
    fn test_resolve() {
        let resolver = PathResolver::new(&test_config());

        let rel = Path::new("corpus/Artist/Album/track.mp3");
        let abs = resolver.resolve(rel);
        assert_eq!(abs, PathBuf::from("/archive/corpus/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_resolve_library() {
        let resolver = PathResolver::new(&test_config());

        let rel = Path::new("libraries/music/Artist/Album/track.mp3");
        let abs = resolver.resolve(rel);
        assert_eq!(
            abs,
            PathBuf::from("/archive/libraries/music/Artist/Album/track.mp3")
        );
    }

    #[test]
    fn test_roundtrip() {
        let resolver = PathResolver::new(&test_config());

        let original = PathBuf::from("/archive/corpus/Artist/Album/track.flac");
        let rel = resolver.to_relative(&original).unwrap();
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
        let resolver = PathResolver::new(&test_config());

        assert_eq!(resolver.root(), Path::new("/archive"));
        assert_eq!(resolver.corpus_dir(), PathBuf::from("/archive/corpus"));
        assert_eq!(
            resolver.libraries_dir(),
            PathBuf::from("/archive/libraries")
        );
        assert_eq!(resolver.stash_dir(), PathBuf::from("/archive/stash"));
    }
}
