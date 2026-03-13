//! Transcode target types.
//!
//! Just the target format enum — actual transcoding logic stays in mm.

use std::path::{Path, PathBuf};

/// Target format for transcoding operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TranscodeTarget {
    /// Opus in OGG container at specified bitrate (VBR).
    Opus { bitrate_kbps: u32 },
    /// FLAC (lossless).
    Flac,
    /// Losslessly capture a lossy source's waveform into FLAC.
    FlacLossyCapture,
}

impl TranscodeTarget {
    /// File extension for the target format (container type).
    pub fn extension(&self) -> &'static str {
        match self {
            TranscodeTarget::Opus { .. } => "opus",
            TranscodeTarget::Flac | TranscodeTarget::FlacLossyCapture => "flac",
        }
    }

    /// Compute the destination path for a transcode of the given source.
    pub fn dest_path(&self, source: &Path) -> PathBuf {
        match self {
            TranscodeTarget::Opus { .. } | TranscodeTarget::Flac => {
                source.with_extension(self.extension())
            }
            TranscodeTarget::FlacLossyCapture => {
                let orig_ext = source.extension().and_then(|e| e.to_str()).unwrap_or("");
                let new_ext = format!("{}.LOSSY.flac", orig_ext);
                source.with_extension(new_ext)
            }
        }
    }
}
