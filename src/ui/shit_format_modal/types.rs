//! Shit Format Resolution Modal Types
//!
//! Data structures for the shit format resolution modal.
//! Separates lossless (remux to FLAC) from lossy (transcode to Opus).

pub use mm_meta::views::cluster_deploy::{ShitFormatEntry, ShitFormatModalData};

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
