//! Observation-phase computation types.
//!
//! Per-file verification computations. Execution logic stays in mm.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::tags::TagSet;

/// A computation that runs during the Observation phase.
///
/// These computations verify individual files against indexed state.
/// They can only spawn other Observation computations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Computation {
    /// Verify tags on disk match database.
    ///
    /// Compares the actual file tags to what's stored in the index.
    /// Pending-write aware: distinguishes MM-initiated writes from external changes.
    VerifyTags {
        inode: i64,
        path: PathBuf,
        /// Watcher-provided disk mtime (avoids stat round-trip).
        mtime_secs: i64,
        mtime_nanos: i64,
        /// Watcher-provided file size.
        file_size: i64,
        /// Watcher-provided disk tags (avoids re-reading file).
        disk_tags: TagSet,
    },

    /// Verify audio stream integrity by decoding the entire file.
    ///
    /// Catches truncated files, corrupt streams, and other audio-level issues
    /// that tag verification wouldn't detect. Emits CorruptFile if decode fails.
    VerifyAudio { inode: i64, path: PathBuf },

    /// Re-run VerifyTags for an inode whose `pending_write` dirty marker is
    /// raised — fires on post-restart initial-scan completion. Reads disk
    /// tags + mtime inline, then dispatches the standard VerifyTags logic.
    /// This unsticks files MM wrote tags to where the post-write watcher
    /// FileChanged event was missed (mtime equality means the watcher's
    /// initial-scan diff sees no change).
    VerifyPendingWrite { inode: i64, path: PathBuf },
}

impl Computation {
    /// Get a human-readable label for this computation.
    pub fn label(&self) -> &'static str {
        match self {
            Computation::VerifyTags { .. } => "Tag verification",
            Computation::VerifyAudio { .. } => "Audio verification",
            Computation::VerifyPendingWrite { .. } => "Tag verification (pending-write recovery)",
        }
    }
}
