//! Shared data types used across computation phases.

use serde::{Deserialize, Serialize};

use crate::db_types::Zone;

/// Enriched metadata for an observed inode (from watcher).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedInodeMeta {
    /// Archive-root-relative path (e.g. "corpus/digital/releases/...")
    pub path: String,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}

/// Image file observed on disk by the watcher thread.
///
/// Contains only FS-level data (no file content reads). Format, dimensions,
/// and role are extracted later by `IndexObservedImages` on a rayon worker,
/// keeping the watcher thread lightweight (stat-only).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ObservedImage {
    pub zone: Zone,
    pub inode: i64,
    pub path: String, // relative to zone root
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}
