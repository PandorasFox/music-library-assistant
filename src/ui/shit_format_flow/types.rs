//! Shit Format Resolution Modal Types
//!
//! Data structures for the shit format resolution modal.
//! Separates lossless (remux to FLAC) from lossy (transcode to Opus).

use std::collections::HashMap;

use anyhow::Result;

use crate::corpus::db::ReadOnlyDb;
use crate::corpus::mutations::Mutation;
use crate::corpus::paths;
use crate::corpus::transcode::TranscodeTarget;

/// Default Opus bitrate in kbps.
const DEFAULT_OPUS_BITRATE: u32 = 128;

/// Minimum Opus bitrate.
const MIN_OPUS_BITRATE: u32 = 32;

/// Maximum Opus bitrate.
const MAX_OPUS_BITRATE: u32 = 512;

/// Bitrate adjustment step.
const BITRATE_STEP: u32 = 8;

/// Lossless formats that can be remuxed to FLAC without quality loss.
const LOSSLESS_FORMATS: &[&str] = &["wav", "aiff", "aif", "ape", "wv"];

/// Lossy formats that need transcoding to Opus.
const LOSSY_FORMATS: &[&str] = &["mp3", "m4a", "aac", "wma"];

/// A file with ShitFormat signal (non-Vorbis container).
#[derive(Debug, Clone)]
pub struct ShitFormatEntry {
    /// Corpus path (relative)
    pub corpus_path: String,
    /// Track ID from tracks table
    pub track_id: i64,
    /// File type (mp3, m4a, etc.)
    pub file_type: String,
}

impl ShitFormatEntry {
    /// Check if this file is a lossless format.
    pub fn is_lossless(&self) -> bool {
        LOSSLESS_FORMATS.contains(&self.file_type.to_lowercase().as_str())
    }

    /// Check if this file is a lossy format.
    pub fn is_lossy(&self) -> bool {
        LOSSY_FORMATS.contains(&self.file_type.to_lowercase().as_str())
    }
}

/// Cached data for the shit format resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
/// Separates files into lossless (remux) and lossy (transcode) categories.
#[derive(Debug, Clone)]
pub struct ShitFormatModalData {
    /// Lossless files (WAV, AIFF, APE, WV) - remux to FLAC
    pub lossless_files: Vec<ShitFormatEntry>,
    /// Lossy files (MP3, M4A, AAC, WMA) - transcode to Opus
    pub lossy_files: Vec<ShitFormatEntry>,
    /// File counts by type (for display breakdown)
    pub file_counts: HashMap<String, i64>,
    /// Opus bitrate in kbps (user-adjustable, for lossy only)
    pub opus_bitrate_kbps: u32,
}

impl Default for ShitFormatModalData {
    fn default() -> Self {
        Self {
            lossless_files: Vec::new(),
            lossy_files: Vec::new(),
            file_counts: HashMap::new(),
            opus_bitrate_kbps: DEFAULT_OPUS_BITRATE,
        }
    }
}

impl ShitFormatModalData {
    /// Load shit format files from the database.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all ShitFormat signals with their file types
        let shit_format_files = read_db.get_shit_format_files()?;
        let file_counts_raw = read_db.get_shit_format_counts_by_type()?;

        if shit_format_files.is_empty() {
            return Ok(Self::default());
        }

        // Build file entries with track info, split by category
        let mut lossless_files = Vec::new();
        let mut lossy_files = Vec::new();

        for (corpus_path, file_type) in shit_format_files {
            // Get track info for this path
            let track = match read_db.get_track_by_path(&corpus_path)? {
                Some(t) => t,
                None => continue, // Signal refers to non-existent track, skip
            };

            let track_id = track.id.unwrap_or(0);

            let entry = ShitFormatEntry {
                corpus_path,
                track_id,
                file_type,
            };

            if entry.is_lossless() {
                lossless_files.push(entry);
            } else if entry.is_lossy() {
                lossy_files.push(entry);
            }
            // Unknown formats are silently ignored
        }

        // Build file counts map
        let file_counts: HashMap<String, i64> = file_counts_raw.into_iter().collect();

        Ok(Self {
            lossless_files,
            lossy_files,
            file_counts,
            opus_bitrate_kbps: DEFAULT_OPUS_BITRATE,
        })
    }

    /// Total number of shit format files.
    pub fn total_count(&self) -> usize {
        self.lossless_files.len() + self.lossy_files.len()
    }

    /// Check if there are lossless files.
    pub fn has_lossless(&self) -> bool {
        !self.lossless_files.is_empty()
    }

    /// Check if there are lossy files.
    pub fn has_lossy(&self) -> bool {
        !self.lossy_files.is_empty()
    }

    /// Get lossless file type breakdown as sorted vec.
    pub fn lossless_breakdown(&self) -> Vec<(&str, i64)> {
        let mut breakdown: Vec<(&str, i64)> = self
            .file_counts
            .iter()
            .filter(|(k, _)| LOSSLESS_FORMATS.contains(&k.to_lowercase().as_str()))
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        breakdown.sort_by(|a, b| b.1.cmp(&a.1));
        breakdown
    }

    /// Get lossy file type breakdown as sorted vec.
    pub fn lossy_breakdown(&self) -> Vec<(&str, i64)> {
        let mut breakdown: Vec<(&str, i64)> = self
            .file_counts
            .iter()
            .filter(|(k, _)| LOSSY_FORMATS.contains(&k.to_lowercase().as_str()))
            .map(|(k, v)| (k.as_str(), *v))
            .collect();
        breakdown.sort_by(|a, b| b.1.cmp(&a.1));
        breakdown
    }

    /// Adjust bitrate (within bounds).
    pub fn adjust_bitrate(&mut self, delta: i32) {
        let new_bitrate = (self.opus_bitrate_kbps as i32 + delta).max(MIN_OPUS_BITRATE as i32).min(MAX_OPUS_BITRATE as i32);
        self.opus_bitrate_kbps = new_bitrate as u32;
    }

    /// Decrease bitrate by one step.
    pub fn decrease_bitrate(&mut self) {
        self.adjust_bitrate(-(BITRATE_STEP as i32));
    }

    /// Increase bitrate by one step.
    pub fn increase_bitrate(&mut self) {
        self.adjust_bitrate(BITRATE_STEP as i32);
    }

    /// Generate Transcode mutations for lossless files only (remux to FLAC).
    pub fn lossless_mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();

        self.lossless_files
            .iter()
            .map(|file| {
                let abs_path = resolver.resolve(std::path::Path::new(&file.corpus_path));

                Mutation::Transcode {
                    track_id: file.track_id,
                    source_path: abs_path,
                    target_format: TranscodeTarget::Flac,
                    stash_name: "originals".to_string(),
                }
            })
            .collect()
    }

    /// Generate Transcode mutations for lossy files only (transcode to Opus).
    pub fn lossy_mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();

        self.lossy_files
            .iter()
            .map(|file| {
                let abs_path = resolver.resolve(std::path::Path::new(&file.corpus_path));

                Mutation::Transcode {
                    track_id: file.track_id,
                    source_path: abs_path,
                    target_format: TranscodeTarget::Opus {
                        bitrate_kbps: self.opus_bitrate_kbps,
                    },
                    stash_name: "originals".to_string(),
                }
            })
            .collect()
    }

    /// Generate Transcode mutations for all files.
    pub fn all_mutations(&self) -> Vec<Mutation> {
        let mut mutations = self.lossless_mutations();
        mutations.extend(self.lossy_mutations());
        mutations
    }
}

/// Which action button is selected in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectedButton {
    /// Remux lossless files to FLAC
    RemuxLossless,
    /// Transcode lossy files to Opus
    TranscodeLossy,
    /// Convert all files
    ConvertAll,
    #[default]
    Cancel,
}

impl SelectedButton {
    /// Cycle to previous button.
    pub fn prev(&mut self, has_lossless: bool, has_lossy: bool) {
        *self = match *self {
            Self::Cancel => {
                if has_lossless && has_lossy {
                    Self::ConvertAll
                } else if has_lossy {
                    Self::TranscodeLossy
                } else if has_lossless {
                    Self::RemuxLossless
                } else {
                    Self::Cancel
                }
            }
            Self::ConvertAll => Self::TranscodeLossy,
            Self::TranscodeLossy => {
                if has_lossless {
                    Self::RemuxLossless
                } else {
                    Self::TranscodeLossy
                }
            }
            Self::RemuxLossless => Self::RemuxLossless,
        };
    }

    /// Cycle to next button.
    pub fn next(&mut self, has_lossless: bool, has_lossy: bool) {
        *self = match *self {
            Self::RemuxLossless => {
                if has_lossy {
                    Self::TranscodeLossy
                } else if has_lossless && has_lossy {
                    Self::ConvertAll
                } else {
                    Self::Cancel
                }
            }
            Self::TranscodeLossy => {
                if has_lossless && has_lossy {
                    Self::ConvertAll
                } else {
                    Self::Cancel
                }
            }
            Self::ConvertAll => Self::Cancel,
            Self::Cancel => Self::Cancel,
        };
    }
}
