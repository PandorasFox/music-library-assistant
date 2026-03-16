//! Inode detail: composite file + audio metadata + tags for wizard pane display.

use serde::{Deserialize, Serialize};

/// Full file detail for wizard pane expansion: audio metadata + DB-cached tags.
///
/// Combines the data from `GetAudioFilesByInodes` (path, file_type, audio info)
/// with `GetCorpusTags` (tag key-value pairs) into a single struct per inode.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InodeDetail {
    pub inode: i64,
    /// Full corpus-relative path.
    pub path: String,
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub file_size: i64,
    /// DB-cached tags as (tag_name, tag_value) pairs.
    pub tags: Vec<(String, String)>,
}
