//! Transcode target types.
//!
//! Just the target format enum — actual transcoding logic stays in mm.

use std::path::{Path, PathBuf};

/// Target format for transcoding operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TranscodeTarget {
    /// FLAC (lossless remux).
    Flac,
}

impl TranscodeTarget {
    /// File extension for the target format (container type).
    pub fn extension(&self) -> &'static str {
        match self {
            TranscodeTarget::Flac => "flac",
        }
    }

    /// Compute the destination path for a transcode of the given source.
    pub fn dest_path(&self, source: &Path) -> PathBuf {
        match self {
            TranscodeTarget::Flac => source.with_extension(self.extension()),
        }
    }
}
