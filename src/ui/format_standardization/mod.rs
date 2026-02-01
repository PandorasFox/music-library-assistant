//! Format Standardization View
//!
//! A lateral view tab for bulk-converting audio files to standardized formats:
//! - Lossy files (MP3, AAC, WMA, M4A) → Opus at configurable bitrate
//! - Lossless files (WAV, AIFF, APE, WV) → FLAC
//!
//! ## Navigation
//!
//! - Up/Down: Select action (Convert Lossy / Convert Lossless)
//! - Left/Right: Adjust Opus bitrate (when lossy is selected)
//! - Enter: Stage selected conversion for execution
//! - Tab/Shift-Tab: Cycle to adjacent view
//! - Esc: Return to main menu

pub mod render;

use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::corpus::db::ReadOnlyDb;

/// Lossy file types that should be converted to Opus.
pub const LOSSY_TYPES: &[&str] = &["mp3", "aac", "wma", "m4a"];

/// Lossless file types that should be converted to FLAC.
pub const LOSSLESS_TYPES: &[&str] = &["wav", "aiff", "aif", "ape", "wv"];

/// Default Opus bitrate in kbps.
const DEFAULT_OPUS_BITRATE: u32 = 128;

/// Minimum Opus bitrate.
const MIN_OPUS_BITRATE: u32 = 32;

/// Maximum Opus bitrate.
const MAX_OPUS_BITRATE: u32 = 512;

/// Bitrate adjustment step.
const BITRATE_STEP: u32 = 8;

/// Action returned from input handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatStdAction {
    None,
    RequestQuit,
    CycleNext,
    CyclePrev,
    /// User confirmed conversion. Contains (is_lossy, opus_bitrate_kbps).
    ConvertLossy { bitrate_kbps: u32 },
    ConvertLossless,
}

/// Which action group is currently selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedAction {
    ConvertLossy,
    ConvertLossless,
}

/// State for the format standardization view.
pub struct FormatStdState {
    /// File counts by file type (cached on view init).
    pub file_counts: HashMap<String, i64>,
    /// Currently selected action group.
    pub selected: SelectedAction,
    /// Opus bitrate in kbps (user-adjustable).
    pub opus_bitrate_kbps: u32,
    /// Whether the Witch is busy (blocks actions).
    pub witch_busy: bool,
}

impl FormatStdState {
    /// Create a new state, querying the database for file counts.
    pub fn new(read_db: &ReadOnlyDb<'_>) -> Self {
        let file_counts = read_db
            .get_audio_type_counts()
            .unwrap_or_default();

        Self {
            file_counts,
            selected: SelectedAction::ConvertLossy,
            opus_bitrate_kbps: DEFAULT_OPUS_BITRATE,
            witch_busy: false,
        }
    }

    /// Get the total count of lossy files that would be converted.
    pub fn lossy_count(&self) -> i64 {
        LOSSY_TYPES
            .iter()
            .map(|t| self.file_counts.get(*t).copied().unwrap_or(0))
            .sum()
    }

    /// Get the total count of lossless files that would be converted.
    pub fn lossless_count(&self) -> i64 {
        LOSSLESS_TYPES
            .iter()
            .map(|t| self.file_counts.get(*t).copied().unwrap_or(0))
            .sum()
    }

    /// Get file counts for each lossy type.
    pub fn lossy_breakdown(&self) -> Vec<(&'static str, i64)> {
        LOSSY_TYPES
            .iter()
            .filter_map(|t| {
                let count = self.file_counts.get(*t).copied().unwrap_or(0);
                if count > 0 {
                    Some((*t, count))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Get file counts for each lossless type.
    pub fn lossless_breakdown(&self) -> Vec<(&'static str, i64)> {
        LOSSLESS_TYPES
            .iter()
            .filter_map(|t| {
                let count = self.file_counts.get(*t).copied().unwrap_or(0);
                if count > 0 {
                    Some((*t, count))
                } else {
                    None
                }
            })
            .collect()
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> FormatStdAction {
        match key.code {
            KeyCode::Esc => FormatStdAction::RequestQuit,

            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    FormatStdAction::CyclePrev
                } else {
                    FormatStdAction::CycleNext
                }
            }
            KeyCode::BackTab => FormatStdAction::CyclePrev,

            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = SelectedAction::ConvertLossy;
                FormatStdAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.selected = SelectedAction::ConvertLossless;
                FormatStdAction::None
            }

            KeyCode::Left | KeyCode::Char('h') => {
                if self.selected == SelectedAction::ConvertLossy {
                    self.opus_bitrate_kbps = (self.opus_bitrate_kbps.saturating_sub(BITRATE_STEP))
                        .max(MIN_OPUS_BITRATE);
                }
                FormatStdAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.selected == SelectedAction::ConvertLossy {
                    self.opus_bitrate_kbps = (self.opus_bitrate_kbps + BITRATE_STEP)
                        .min(MAX_OPUS_BITRATE);
                }
                FormatStdAction::None
            }

            KeyCode::Enter => {
                if self.witch_busy {
                    return FormatStdAction::None;
                }
                match self.selected {
                    SelectedAction::ConvertLossy => {
                        if self.lossy_count() > 0 {
                            FormatStdAction::ConvertLossy {
                                bitrate_kbps: self.opus_bitrate_kbps,
                            }
                        } else {
                            FormatStdAction::None
                        }
                    }
                    SelectedAction::ConvertLossless => {
                        if self.lossless_count() > 0 {
                            FormatStdAction::ConvertLossless
                        } else {
                            FormatStdAction::None
                        }
                    }
                }
            }

            _ => FormatStdAction::None,
        }
    }
}
