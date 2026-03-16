//! Lossless Remux Resolution Modal
//!
//! Provides the interactive workflow for remuxing non-Vorbis lossless container
//! format files (WAV, AIFF, APE, WV) to FLAC.
//!
//! Launched from Insights when LosslessRemux signals are present.

pub mod preview;
pub mod types;

pub use types::{LosslessRemuxAction, LosslessRemuxModalData, LosslessRemuxPreviewState};
