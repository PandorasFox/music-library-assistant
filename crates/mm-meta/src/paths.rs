//! Path Resolution Layer
//!
//! Handles conversion between absolute filesystem paths and root-relative database paths.

use std::path::{Path, PathBuf};

/// Resolves paths between absolute filesystem paths and root-relative database paths.
///
/// Created from Config at startup, stored in Witch for access throughout the app.
#[derive(Debug, Clone)]
pub struct PathResolver {
    root: PathBuf,
}

impl PathResolver {
    /// Create a new PathResolver from an archive root path.
    pub fn from_root(root: PathBuf) -> Self {
        Self { root }
    }

    /// Create a new PathResolver from a Config.
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self::from_root(config.root.clone())
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
    pub fn stash_dir(&self) -> PathBuf {
        self.root.join("stash")
    }

    /// Get the inbox directory path.
    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }
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
pub fn is_stash_path(rel: &Path) -> bool {
    rel.starts_with("stash")
}

/// Extract modification time from metadata as (seconds, nanoseconds) tuple.
pub fn read_mtime(metadata: &std::fs::Metadata) -> (i64, i64) {
    use std::time::UNIX_EPOCH;
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| (d.as_secs() as i64, d.subsec_nanos() as i64))
        .unwrap_or((0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_utils::t;

    #[test]
    fn test_to_relative() {
        let resolver = PathResolver::from_root(PathBuf::from("/archive"));

        let abs = Path::new("/archive/corpus/Artist/Album/track.mp3");
        let rel = t!(resolver.to_relative(abs));
        assert_eq!(rel, PathBuf::from("corpus/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_to_relative_mismatch() {
        let resolver = PathResolver::from_root(PathBuf::from("/archive"));

        let abs = Path::new("/other/path/track.mp3");
        assert!(resolver.to_relative(abs).is_none());
    }

    #[test]
    fn test_resolve() {
        let resolver = PathResolver::from_root(PathBuf::from("/archive"));

        let rel = Path::new("corpus/Artist/Album/track.mp3");
        let abs = resolver.resolve(rel);
        assert_eq!(abs, PathBuf::from("/archive/corpus/Artist/Album/track.mp3"));
    }

    #[test]
    fn test_classification() {
        assert!(is_corpus_path(Path::new("corpus/Artist/Album/track.flac")));
        assert!(!is_corpus_path(Path::new("libraries/music/track.mp3")));

        assert!(is_library_path(Path::new("libraries/music/track.mp3")));
        assert!(!is_library_path(Path::new("corpus/track.flac")));

        assert!(is_stash_path(Path::new("stash/cleanup/track.flac")));
        assert!(!is_stash_path(Path::new("corpus/track.flac")));
    }
}
