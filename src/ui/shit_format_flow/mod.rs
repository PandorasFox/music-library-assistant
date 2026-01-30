//! Shit Format Resolution Flow UI Module
//!
//! Provides the interactive workflow for resolving non-Vorbis container format files.
//! Shows files with ShitFormat signals (MP3, M4A, AAC, WMA, etc.) and allows
//! the operator to transcode them to Opus with a configurable bitrate.
//!
//! Reuses patterns from FormatStandardization but as a modal launched from Insights.

pub mod preview;
pub mod types;

pub use preview::{ShitFormatPreviewAction, ShitFormatPreviewState};
pub use types::ShitFormatModalData;
