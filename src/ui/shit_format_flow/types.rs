//! Shit Format Resolution Modal Types
//!
//! Data structures for the shit format resolution modal.
//! Similar to FormatStandardization but modal-specific.

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

/// Cached data for the shit format resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone)]
pub struct ShitFormatModalData {
    /// Files with ShitFormat signals
    pub files: Vec<ShitFormatEntry>,
    /// File counts by type (for display breakdown)
    pub file_counts: HashMap<String, i64>,
    /// Opus bitrate in kbps (user-adjustable)
    pub opus_bitrate_kbps: u32,
}

impl Default for ShitFormatModalData {
    fn default() -> Self {
        Self {
            files: Vec::new(),
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

        // Build file entries with track info
        let mut files = Vec::new();
        for (corpus_path, file_type) in shit_format_files {
            // Get track info for this path
            let track = match read_db.get_track_by_path(&corpus_path)? {
                Some(t) => t,
                None => continue, // Signal refers to non-existent track, skip
            };

            let track_id = track.id.unwrap_or(0);

            files.push(ShitFormatEntry {
                corpus_path,
                track_id,
                file_type,
            });
        }

        // Build file counts map
        let file_counts: HashMap<String, i64> = file_counts_raw.into_iter().collect();

        Ok(Self {
            files,
            file_counts,
            opus_bitrate_kbps: DEFAULT_OPUS_BITRATE,
        })
    }

    /// Total number of shit format files.
    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    /// Check if there are any shit format files.
    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Get file type breakdown as sorted vec.
    pub fn type_breakdown(&self) -> Vec<(&str, i64)> {
        let mut breakdown: Vec<(&str, i64)> = self
            .file_counts
            .iter()
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

    /// Generate Transcode mutations for all files.
    ///
    /// Transcodes to Opus with the configured bitrate.
    /// Stashing original is handled by the Transcode executor.
    pub fn transcode_mutations(&self) -> Vec<Mutation> {
        let resolver = paths::get_resolver();

        self.files
            .iter()
            .map(|file| {
                // Resolve relative path to absolute for filesystem operations
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
}

/// Which action button is selected in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectedButton {
    TranscodeAll,
    #[default]
    Cancel,
}

impl SelectedButton {
    /// Move selection left.
    pub fn left(&mut self, has_files: bool) {
        *self = match *self {
            Self::Cancel => {
                if has_files {
                    Self::TranscodeAll
                } else {
                    Self::Cancel
                }
            }
            Self::TranscodeAll => Self::TranscodeAll,
        };
    }

    /// Move selection right.
    pub fn right(&mut self, _has_files: bool) {
        *self = match *self {
            Self::TranscodeAll => Self::Cancel,
            Self::Cancel => Self::Cancel,
        };
    }
}
