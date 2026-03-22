//! Path Resolution Layer
//!
//! Two kinds of relative paths:
//!
//! - **Root-relative**: `corpus/Artist/Album/track.flac` — includes zone directory,
//!   produced by `to_relative()`. Used for routing (classifying which zone a path
//!   belongs to). NOT stored in the database.
//!
//! - **Zone-relative**: `Artist/Album/track.flac` — relative to the zone root,
//!   produced by `to_zone_relative()`. This is the format stored in the DB and
//!   signal tables. The zone is metadata on the row, not part of the path.

use std::path::{Path, PathBuf};

use crate::db_types::Zone;

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

    /// Convert an absolute path to a root-relative path (includes zone directory).
    ///
    /// Returns e.g. `corpus/Artist/Album/track.flac`. Used for routing — do NOT
    /// store the result in the DB. Use `to_zone_relative()` for DB paths.
    pub fn to_relative(&self, abs: &Path) -> Option<PathBuf> {
        abs.strip_prefix(&self.root).ok().map(PathBuf::from)
    }

    /// Convert an absolute path to a zone-relative path for DB storage.
    ///
    /// Strips both the archive root and the zone directory.
    /// Returns e.g. `Artist/Album/track.flac` for a corpus file.
    pub fn to_zone_relative(&self, abs: &Path, zone: Zone) -> Option<PathBuf> {
        abs.strip_prefix(&self.zone_dir(zone)).ok().map(PathBuf::from)
    }

    /// Resolve a root-relative path to an absolute filesystem path.
    ///
    /// Input should include the zone directory (e.g. `corpus/Artist/Album/track.flac`).
    /// For zone-relative DB paths, use `resolve_for_zone()` instead.
    pub fn resolve(&self, rel: &Path) -> PathBuf {
        self.root.join(rel)
    }

    /// Resolve a zone-relative DB path to an absolute filesystem path.
    pub fn resolve_for_zone(&self, zone: Zone, rel: &Path) -> PathBuf {
        self.zone_dir(zone).join(rel)
    }

    /// Get the filesystem directory for a zone.
    pub fn zone_dir(&self, zone: Zone) -> PathBuf {
        match zone {
            Zone::Corpus => self.corpus_dir(),
            Zone::Library => self.libraries_dir(),
            Zone::Inbox => self.inbox_dir(),
        }
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
///
/// Operates on root-relative paths from `to_relative()`, NOT DB paths.
pub fn is_corpus_path(rel: &Path) -> bool {
    rel.starts_with("corpus")
}

/// Check if a root-relative path is a library path.
///
/// Operates on root-relative paths from `to_relative()`, NOT DB paths.
pub fn is_library_path(rel: &Path) -> bool {
    rel.starts_with("libraries")
}

/// Check if a root-relative path is a stash path.
///
/// Operates on root-relative paths from `to_relative()`, NOT DB paths.
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

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            root: PathBuf::from("/archive"),
            legacy_enabled: true,
            source_dirs: vec![],
            opinions: Default::default(),
        }
    }

    #[test]
    fn test_to_zone_relative() {
        let resolver = PathResolver::from_config(&test_config());

        let abs = Path::new("/archive/corpus/Artist/Album/track.flac");
        let rel = t!(resolver.to_zone_relative(abs, Zone::Corpus));
        assert_eq!(rel, PathBuf::from("Artist/Album/track.flac"));

        let abs = Path::new("/archive/libraries/music/Artist/track.opus");
        let rel = t!(resolver.to_zone_relative(abs, Zone::Library));
        assert_eq!(rel, PathBuf::from("music/Artist/track.opus"));

        // Wrong zone returns None
        let abs = Path::new("/archive/corpus/Artist/Album/track.flac");
        assert!(resolver.to_zone_relative(abs, Zone::Library).is_none());
    }

    #[test]
    fn test_resolve_for_zone() {
        let resolver = PathResolver::from_config(&test_config());

        let abs = resolver.resolve_for_zone(Zone::Corpus, Path::new("Artist/Album/track.flac"));
        assert_eq!(abs, PathBuf::from("/archive/corpus/Artist/Album/track.flac"));

        let abs = resolver.resolve_for_zone(Zone::Library, Path::new("music/Artist/track.opus"));
        assert_eq!(abs, PathBuf::from("/archive/libraries/music/Artist/track.opus"));

        let abs = resolver.resolve_for_zone(Zone::Inbox, Path::new("unsorted/track.mp3"));
        assert_eq!(abs, PathBuf::from("/archive/inbox/unsorted/track.mp3"));
    }

}
