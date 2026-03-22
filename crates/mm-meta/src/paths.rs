//! Path Resolution Layer
//!
//! Multi-root architecture: `storage_root` IS the corpus directory (no subdirectory),
//! libraries and stash can live anywhere on the same filesystem.
//!
//! Two kinds of relative paths:
//!
//! - **Root-relative**: `corpus/Artist/Album/track.flac` — includes synthetic zone
//!   prefix, produced by `to_relative()`. Used for routing (classifying which zone
//!   a path belongs to). NOT stored in the database.
//!
//! - **Zone-relative**: `Artist/Album/track.flac` — relative to the zone root,
//!   produced by `to_zone_relative()`. This is the format stored in the DB and
//!   signal tables. The zone is metadata on the row, not part of the path.

use std::path::{Path, PathBuf};

use crate::db_types::Zone;

/// Resolves paths between absolute filesystem paths and zone-relative database paths.
///
/// Constructed from Config at startup. Each root is fully resolved (no Option).
#[derive(Debug, Clone)]
pub struct PathResolver {
    /// The corpus directory (== Config.storage_root).
    storage_root: PathBuf,
    /// Resolved libraries root.
    libraries_root: PathBuf,
    /// Resolved stash root.
    stash_root: PathBuf,
}

impl PathResolver {
    /// Create a new PathResolver with explicit roots.
    pub fn new(storage_root: PathBuf, libraries_root: PathBuf, stash_root: PathBuf) -> Self {
        Self {
            storage_root,
            libraries_root,
            stash_root,
        }
    }

    /// Create from an archive root path (single-root legacy layout).
    pub fn from_root(root: PathBuf) -> Self {
        let libraries_root = root.join("libraries");
        let stash_root = root.join("stash");
        // In legacy layout, corpus was root/corpus. In multi-root, storage_root IS corpus.
        // But from_root is used by old callers expecting root/corpus layout. Keep for compat
        // during transition — new code uses from_config().
        Self {
            storage_root: root.join("corpus"),
            libraries_root,
            stash_root,
        }
    }

    /// Create a new PathResolver from a Config.
    pub fn from_config(config: &crate::config::Config) -> Self {
        Self::new(
            config.storage_root.clone(),
            config.libraries_dir(),
            config.stash_dir(),
        )
    }

    /// Convert an absolute path to a root-relative path with synthetic zone prefix.
    ///
    /// Returns e.g. `corpus/Artist/Album/track.flac`. Used for routing — do NOT
    /// store the result in the DB. Use `to_zone_relative()` for DB paths.
    ///
    /// Tries each root in order: stash, libraries, storage (longest-prefix first
    /// to handle nested case where libraries_root is under storage_root).
    pub fn to_relative(&self, abs: &Path) -> Option<PathBuf> {
        // Check stash first (may be under storage_root)
        if let Ok(rel) = abs.strip_prefix(&self.stash_root) {
            return Some(Path::new("stash").join(rel));
        }
        // Then libraries (may be under storage_root)
        if let Ok(rel) = abs.strip_prefix(&self.libraries_root) {
            return Some(Path::new("libraries").join(rel));
        }
        // Then corpus (storage_root)
        if let Ok(rel) = abs.strip_prefix(&self.storage_root) {
            return Some(Path::new("corpus").join(rel));
        }
        None
    }

    /// Convert an absolute path to a zone-relative path for DB storage.
    ///
    /// Strips the zone's root directory.
    /// Returns e.g. `Artist/Album/track.flac` for a corpus file.
    pub fn to_zone_relative(&self, abs: &Path, zone: Zone) -> Option<PathBuf> {
        abs.strip_prefix(&self.zone_dir(zone)).ok().map(PathBuf::from)
    }

    /// Resolve a root-relative path (with synthetic zone prefix) to absolute.
    ///
    /// Input should include the zone prefix (e.g. `corpus/Artist/Album/track.flac`).
    /// For zone-relative DB paths, use `resolve_for_zone()` instead.
    pub fn resolve(&self, rel: &Path) -> PathBuf {
        if let Ok(rest) = rel.strip_prefix("corpus") {
            self.storage_root.join(rest)
        } else if let Ok(rest) = rel.strip_prefix("libraries") {
            self.libraries_root.join(rest)
        } else if let Ok(rest) = rel.strip_prefix("stash") {
            self.stash_root.join(rest)
        } else {
            // Fallback: treat as corpus-relative (backwards compat for DB paths)
            self.storage_root.join(rel)
        }
    }

    /// Resolve a zone-relative DB path to an absolute filesystem path.
    pub fn resolve_for_zone(&self, zone: Zone, rel: &Path) -> PathBuf {
        self.zone_dir(zone).join(rel)
    }

    /// Get the filesystem directory for a zone.
    pub fn zone_dir(&self, zone: Zone) -> PathBuf {
        match zone {
            Zone::Corpus => self.storage_root.clone(),
            Zone::Library => self.libraries_root.clone(),
        }
    }

    /// Get the corpus directory path (== storage_root).
    pub fn corpus_dir(&self) -> PathBuf {
        self.storage_root.clone()
    }

    /// Get the libraries directory path.
    pub fn libraries_dir(&self) -> PathBuf {
        self.libraries_root.clone()
    }

    /// Get the stash directory path.
    pub fn stash_dir(&self) -> PathBuf {
        self.stash_root.clone()
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

    fn test_config() -> crate::config::Config {
        crate::config::Config {
            storage_root: PathBuf::from("/archive/audio"),
            libraries_root: None,
            stash_root: None,
            source_dirs: vec![],
            opinions: Default::default(),
        }
    }

    fn test_config_multi_root() -> crate::config::Config {
        crate::config::Config {
            storage_root: PathBuf::from("/mnt/pool/libraries/archive/audio"),
            libraries_root: Some(PathBuf::from("/mnt/pool/libraries")),
            stash_root: Some(PathBuf::from("/mnt/pool/stash")),
            source_dirs: vec![],
            opinions: Default::default(),
        }
    }

    #[test]
    fn test_to_relative_default_layout() {
        let resolver = PathResolver::from_config(&test_config());

        let abs = Path::new("/archive/audio/Artist/Album/track.mp3");
        let rel = t!(resolver.to_relative(abs));
        assert_eq!(rel, PathBuf::from("corpus/Artist/Album/track.mp3"));

        let abs = Path::new("/archive/audio/libraries/music/track.mp3");
        let rel = t!(resolver.to_relative(abs));
        assert_eq!(rel, PathBuf::from("libraries/music/track.mp3"));
    }

    #[test]
    fn test_to_relative_multi_root() {
        let resolver = PathResolver::from_config(&test_config_multi_root());

        let abs = Path::new("/mnt/pool/libraries/archive/audio/Artist/Album/track.flac");
        let rel = t!(resolver.to_relative(abs));
        assert_eq!(rel, PathBuf::from("corpus/Artist/Album/track.flac"));

        let abs = Path::new("/mnt/pool/libraries/music/track.opus");
        let rel = t!(resolver.to_relative(abs));
        assert_eq!(rel, PathBuf::from("libraries/music/track.opus"));

        let abs = Path::new("/mnt/pool/stash/dupes/track.flac");
        let rel = t!(resolver.to_relative(abs));
        assert_eq!(rel, PathBuf::from("stash/dupes/track.flac"));
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
        assert_eq!(abs, PathBuf::from("/archive/audio/Artist/Album/track.mp3"));

        let rel = Path::new("libraries/music/track.opus");
        let abs = resolver.resolve(rel);
        assert_eq!(abs, PathBuf::from("/archive/audio/libraries/music/track.opus"));
    }

    #[test]
    fn test_resolve_multi_root() {
        let resolver = PathResolver::from_config(&test_config_multi_root());

        let rel = Path::new("corpus/Artist/Album/track.mp3");
        let abs = resolver.resolve(rel);
        assert_eq!(abs, PathBuf::from("/mnt/pool/libraries/archive/audio/Artist/Album/track.mp3"));

        let rel = Path::new("libraries/music/track.opus");
        let abs = resolver.resolve(rel);
        assert_eq!(abs, PathBuf::from("/mnt/pool/libraries/music/track.opus"));

        let rel = Path::new("stash/dupes/track.flac");
        let abs = resolver.resolve(rel);
        assert_eq!(abs, PathBuf::from("/mnt/pool/stash/dupes/track.flac"));
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

    #[test]
    fn test_to_zone_relative() {
        let resolver = PathResolver::from_config(&test_config());

        let abs = Path::new("/archive/audio/Artist/Album/track.flac");
        let rel = t!(resolver.to_zone_relative(abs, Zone::Corpus));
        assert_eq!(rel, PathBuf::from("Artist/Album/track.flac"));

        let abs = Path::new("/archive/audio/libraries/music/Artist/track.opus");
        let rel = t!(resolver.to_zone_relative(abs, Zone::Library));
        assert_eq!(rel, PathBuf::from("music/Artist/track.opus"));

        // Wrong zone returns None
        let abs = Path::new("/archive/audio/Artist/Album/track.flac");
        assert!(resolver.to_zone_relative(abs, Zone::Library).is_none());
    }

    #[test]
    fn test_resolve_for_zone() {
        let resolver = PathResolver::from_config(&test_config());

        let abs = resolver.resolve_for_zone(Zone::Corpus, Path::new("Artist/Album/track.flac"));
        assert_eq!(abs, PathBuf::from("/archive/audio/Artist/Album/track.flac"));

        let abs = resolver.resolve_for_zone(Zone::Library, Path::new("music/Artist/track.opus"));
        assert_eq!(abs, PathBuf::from("/archive/audio/libraries/music/Artist/track.opus"));
    }

}
