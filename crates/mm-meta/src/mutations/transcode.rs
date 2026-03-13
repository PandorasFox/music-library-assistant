//! Transcode mutation struct.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::transcode::TranscodeTarget;

/// Transcode a file to a different container/codec format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TranscodeMutation {
    pub inode: i64,
    pub source_path: PathBuf,
    pub target_format: TranscodeTarget,
    pub stash_name: String,
}
