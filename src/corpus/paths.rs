//! Path Resolution Layer
//!
//! Handles conversion between absolute filesystem paths and relative database paths.
//! All paths in the database are stored relative to their respective roots:
//! - Corpus files: relative to `corpus_root`
//! - Legacy files: relative to `legacy_library` (if configured)
//! - Library files: relative to `libraries_root`
//!
//! This enables archive relocation without database modification.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::config::{self, Config};

/// Global PathResolver for use throughout the codebase.
/// Initialized lazily on first use from config.
static GLOBAL_RESOLVER: OnceLock<PathResolver> = OnceLock::new();

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

/// Resolves paths between absolute filesystem paths and relative database paths.
///
/// Created from Config at startup, stored in Witch for access throughout the app.
#[derive(Debug, Clone)]
pub struct PathResolver {
    corpus_root: PathBuf,
    libraries_root: PathBuf,
    legacy_library: Option<PathBuf>,
}

impl PathResolver {
    /// Create a new PathResolver from configuration.
    pub fn new(config: &Config) -> Self {
        Self {
            corpus_root: config.corpus_root.clone(),
            libraries_root: config.libraries_root.clone(),
            legacy_library: config.legacy_library.clone(),
        }
    }

    // =========================================================================
    // Absolute → Relative (for storage)
    // =========================================================================

    /// Convert an absolute corpus path to a relative path for database storage.
    ///
    /// Returns None if the path doesn't start with corpus_root.
    pub fn to_relative_corpus(&self, abs: &Path) -> Option<PathBuf> {
        abs.strip_prefix(&self.corpus_root).ok().map(PathBuf::from)
    }

    /// Convert an absolute library path to a relative path for database storage.
    ///
    /// Returns None if the path doesn't start with libraries_root.
    /// The returned path includes the library subdirectory (e.g., "music/Artist/Album/track.mp3").
    pub fn to_relative_library(&self, abs: &Path) -> Option<PathBuf> {
        abs.strip_prefix(&self.libraries_root).ok().map(PathBuf::from)
    }

    /// Convert an absolute legacy library path to a relative path for database storage.
    ///
    /// Returns None if legacy_library is not configured or path doesn't match.
    pub fn to_relative_legacy(&self, abs: &Path) -> Option<PathBuf> {
        self.legacy_library
            .as_ref()
            .and_then(|root| abs.strip_prefix(root).ok().map(PathBuf::from))
    }

    /// Convert an absolute path to relative, using the source to determine which root.
    ///
    /// Source mapping:
    /// - "corpus" → corpus_root
    /// - "legacy" → legacy_library
    /// - anything else → libraries_root (library names are subdirs)
    pub fn to_relative(&self, abs: &Path, source: &str) -> Option<PathBuf> {
        match source {
            "corpus" => self.to_relative_corpus(abs),
            "legacy" => self.to_relative_legacy(abs),
            _ => self.to_relative_library(abs),
        }
    }

    // =========================================================================
    // Relative → Absolute (for use)
    // =========================================================================

    /// Resolve a relative corpus path to an absolute filesystem path.
    pub fn resolve_corpus(&self, rel: &Path) -> PathBuf {
        self.corpus_root.join(rel)
    }

    /// Resolve a relative library path to an absolute filesystem path.
    ///
    /// The relative path should include the library subdirectory
    /// (e.g., "music/Artist/Album/track.mp3").
    pub fn resolve_library(&self, rel: &Path) -> PathBuf {
        self.libraries_root.join(rel)
    }

    /// Resolve a relative legacy path to an absolute filesystem path.
    ///
    /// Returns None if legacy_library is not configured.
    pub fn resolve_legacy(&self, rel: &Path) -> Option<PathBuf> {
        self.legacy_library.as_ref().map(|root| root.join(rel))
    }

    /// Resolve a relative path to absolute, using the source to determine which root.
    ///
    /// Returns None if:
    /// - source is "legacy" but legacy_library is not configured
    pub fn resolve(&self, rel: &Path, source: &str) -> Option<PathBuf> {
        match source {
            "corpus" => Some(self.resolve_corpus(rel)),
            "legacy" => self.resolve_legacy(rel),
            _ => Some(self.resolve_library(rel)),
        }
    }

    // =========================================================================
    // Accessors (for migration and display)
    // =========================================================================

    /// Get the corpus root path.
    pub fn corpus_root(&self) -> &Path {
        &self.corpus_root
    }

    /// Get the libraries root path.
    pub fn libraries_root(&self) -> &Path {
        &self.libraries_root
    }

    /// Get the legacy library path, if configured.
    pub fn legacy_library(&self) -> Option<&Path> {
        self.legacy_library.as_deref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> Config {
        Config {
            corpus_root: PathBuf::from("/archive/corpus"),
            libraries_root: PathBuf::from("/archive/libraries"),
            legacy_library: Some(PathBuf::from("/archive/legacy")),
            deploy_mappings: vec![],
            stash_dir: None,
            opinions: Default::default(),
        }
    }

    #[test]
    fn test_to_relative_corpus() {
        let resolver = PathResolver::new(&test_config());

        let abs = Path::new("/archive/corpus/Artist/Album/track.mp3");
        let rel = resolver.to_relative_corpus(abs).unwrap();
        assert_eq!(rel, PathBuf::from("Artist/Album/track.mp3"));
    }

    #[test]
    fn test_to_relative_corpus_mismatch() {
        let resolver = PathResolver::new(&test_config());

        let abs = Path::new("/other/path/track.mp3");
        assert!(resolver.to_relative_corpus(abs).is_none());
    }

    #[test]
    fn test_resolve_corpus() {
        let resolver = PathResolver::new(&test_config());

        let rel = Path::new("Artist/Album/track.mp3");
        let abs = resolver.resolve_corpus(rel);
        assert_eq!(abs, PathBuf::from("/archive/corpus/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_to_relative_library() {
        let resolver = PathResolver::new(&test_config());

        let abs = Path::new("/archive/libraries/music/Artist/Album/track.mp3");
        let rel = resolver.to_relative_library(abs).unwrap();
        assert_eq!(rel, PathBuf::from("music/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_resolve_library() {
        let resolver = PathResolver::new(&test_config());

        let rel = Path::new("music/Artist/Album/track.mp3");
        let abs = resolver.resolve_library(rel);
        assert_eq!(abs, PathBuf::from("/archive/libraries/music/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_legacy_with_config() {
        let resolver = PathResolver::new(&test_config());

        let abs = Path::new("/archive/legacy/old/track.mp3");
        let rel = resolver.to_relative_legacy(abs).unwrap();
        assert_eq!(rel, PathBuf::from("old/track.mp3"));

        let resolved = resolver.resolve_legacy(&rel).unwrap();
        assert_eq!(resolved, abs);
    }

    #[test]
    fn test_legacy_without_config() {
        let mut config = test_config();
        config.legacy_library = None;
        let resolver = PathResolver::new(&config);

        let abs = Path::new("/archive/legacy/old/track.mp3");
        assert!(resolver.to_relative_legacy(abs).is_none());
        assert!(resolver.resolve_legacy(Path::new("old/track.mp3")).is_none());
    }

    #[test]
    fn test_source_aware_resolution() {
        let resolver = PathResolver::new(&test_config());

        // Corpus
        let rel = Path::new("Artist/Album/track.mp3");
        let abs = resolver.resolve(rel, "corpus").unwrap();
        assert_eq!(abs, PathBuf::from("/archive/corpus/Artist/Album/track.mp3"));

        // Legacy
        let abs = resolver.resolve(rel, "legacy").unwrap();
        assert_eq!(abs, PathBuf::from("/archive/legacy/Artist/Album/track.mp3"));

        // Library (any other source name)
        let abs = resolver.resolve(rel, "music").unwrap();
        assert_eq!(abs, PathBuf::from("/archive/libraries/Artist/Album/track.mp3"));
    }
}
