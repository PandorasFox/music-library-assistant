//! FFmpeg-based audio transcoding.
//!
//! Provides subprocess wrappers for transcoding audio files to standardized formats.
//! Requires ffmpeg to be installed and available in PATH.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Target format for transcoding operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum TranscodeTarget {
    /// Opus in OGG container at specified bitrate (VBR).
    Opus { bitrate_kbps: u32 },
    /// FLAC (lossless).
    Flac,
}

impl TranscodeTarget {
    /// File extension for the target format.
    pub fn extension(&self) -> &'static str {
        match self {
            TranscodeTarget::Opus { .. } => "opus",
            TranscodeTarget::Flac => "flac",
        }
    }

    /// Human-readable label for display.
    pub fn label(&self) -> String {
        match self {
            TranscodeTarget::Opus { bitrate_kbps } => format!("Opus {}kbps", bitrate_kbps),
            TranscodeTarget::Flac => "FLAC".to_string(),
        }
    }
}

/// Transcode a source audio file to the target format.
///
/// The destination path must not already exist. FFmpeg is invoked as a subprocess.
pub fn transcode(source: &Path, dest: &Path, target: TranscodeTarget) -> Result<()> {
    if !source.exists() {
        return Err(anyhow::anyhow!(
            "Source file does not exist: {}",
            source.display()
        ));
    }

    if dest.exists() {
        return Err(anyhow::anyhow!(
            "Destination file already exists: {}",
            dest.display()
        ));
    }

    // Create parent directory if needed
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create directory: {}", parent.display()))?;
    }

    let source_str = source
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Source path is not valid UTF-8: {}", source.display()))?;
    let dest_str = dest
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Dest path is not valid UTF-8: {}", dest.display()))?;

    let bitrate_string;
    let mut args = vec!["-i", source_str, "-vn"];

    match target {
        TranscodeTarget::Opus { bitrate_kbps } => {
            bitrate_string = format!("{}k", bitrate_kbps);
            args.extend_from_slice(&["-c:a", "libopus", "-b:a", &bitrate_string]);
        }
        TranscodeTarget::Flac => {
            args.extend_from_slice(&["-c:a", "flac"]);
        }
    }

    args.push(dest_str);

    let output = Command::new("ffmpeg")
        .args(&args)
        .output()
        .context("Failed to execute ffmpeg. Is ffmpeg installed and in PATH?")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow::anyhow!(
            "ffmpeg failed (exit {}): {}",
            output.status.code().unwrap_or(-1),
            stderr.lines().last().unwrap_or("unknown error")
        ));
    }

    // Verify the output file was created
    if !dest.exists() {
        return Err(anyhow::anyhow!(
            "ffmpeg completed but output file was not created: {}",
            dest.display()
        ));
    }

    Ok(())
}
