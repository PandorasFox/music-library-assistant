//! Deploy lifecycle signal types.

use std::hash::Hash;

use crate::meta::signals::registry::SignalContentHash;

// Re-export pure data types from mm-meta
pub use mm_meta::signals::data::SidecarDeployReadyData;

// ============================================================================
// Deploy signals (inode-keyed)
// ============================================================================

/// Healthy corpus file ready for deployment (not yet in any library).
#[derive(Debug, Clone)]
pub struct DeployReadySignal {
    pub inode: i64,
    pub path: String,
    pub deploy_path: String,
}

/// Healthy corpus file deployed at correct library path.
#[derive(Debug, Clone)]
pub struct DeployedHealthySignal {
    pub inode: i64,
    pub path: String,
    pub library_path: String,
}

/// Corpus sidecar image file ready for deployment to a library.
///
/// Emitted by `DeriveCorpusDeployStatus` for image files in corpus directories
/// that have deployed or deploy-ready audio, where the corresponding library
/// directory does not yet contain the image.
#[derive(Debug, Clone)]
pub struct SidecarDeployReadySignal {
    pub inode: i64,
    /// Corpus image path
    pub path: String,
    /// Target deploy path (relative, no library prefix): "Artist/Album/cover.jpg"
    pub deploy_path: String,
    /// Target library name
    pub library_name: String,
    /// Image metadata stored as BLOB
    pub data: SidecarDeployReadyData,
}

// ============================================================================
// Deploy aggregate signals (key-keyed)
// ============================================================================

/// Library file without corpus backing.
#[derive(Debug, Clone)]
pub struct LibraryLeftoverSignal {
    pub key: String,
}

impl_library_keyed!(LibraryLeftoverSignal, "library_leftover");

impl LibraryLeftoverSignal {
    /// Parse a key into (library_name, library_path). Returns None if malformed.
    pub fn parse_key(key: &str) -> Option<(&str, &str)> {
        let after = key.strip_prefix("library_leftover:")?;
        let idx = after.find(':')?;
        Some((&after[..idx], &after[idx + 1..]))
    }
}

/// Library file at wrong path (tags changed since deploy).
#[derive(Debug, Clone)]
pub struct LibraryStaleSignal {
    pub key: String,
    pub library_path: String,
    pub expected_path: String,
    pub corpus_path: String,
    pub inode: i64,
}

impl_library_keyed!(LibraryStaleSignal, "library_stale");

/// Multiple corpus files deploy to the same library path.
#[derive(Debug, Clone)]
pub struct DeployConflictSignal {
    pub key: String,
    pub inodes: Vec<i64>,
}

/// Multiple corpus image files deploy to the same sidecar library path.
#[derive(Debug, Clone)]
pub struct SidecarDeployConflictSignal {
    /// Key: "library_name/deploy_path" (e.g. "music/Artist/Album/cover.jpg")
    pub key: String,
    /// The library-relative deploy path (e.g. "Artist/Album/cover.jpg")
    pub deploy_path: String,
    /// Target library name
    pub library_name: String,
    /// All corpus image inodes that would deploy to this path
    pub inodes: Vec<i64>,
}

// ============================================================================
// impl_content_hash! invocations for deploy signals
// ============================================================================

impl_content_hash!(DeployReadySignal => [path, deploy_path]);
impl_content_hash!(DeployedHealthySignal => [path, library_path]);
impl_content_hash!(SidecarDeployReadySignal => [path, deploy_path, library_name] + blob(data));
impl_content_hash!(LibraryLeftoverSignal => []);
impl_content_hash!(LibraryStaleSignal => [library_path, expected_path, corpus_path, inode]);
impl_content_hash!(DeployConflictSignal => blob(inodes));
impl_content_hash!(SidecarDeployConflictSignal => blob(inodes));
