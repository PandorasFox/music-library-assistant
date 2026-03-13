//! Shit Format Resolution Modal Types
//!
//! Data structures for the shit format resolution modal.
//! Separates lossless (remux to FLAC) from lossy (transcode to Opus).

pub use mm_meta::views::cluster_deploy::{ShitFormatEntry, ShitFormatModalData};

use std::borrow::Cow;

use ratatui::style::Color;

use crate::widgets::modal_buttons::ModalButtons;

use super::preview::ShitFormatPreviewAction;

/// Button choices for the shit format resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShitFormatButton {
    /// Remux lossless files to FLAC
    RemuxLossless,
    /// Transcode lossy files to Opus
    TranscodeLossy,
    /// Convert all files
    ConvertAll,
    #[default]
    Cancel,
}

impl ModalButtons for ShitFormatButton {
    type Context = ShitFormatModalData;
    type Action = ShitFormatPreviewAction;

    fn all() -> &'static [Self] {
        &[Self::RemuxLossless, Self::TranscodeLossy, Self::ConvertAll, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::RemuxLossless => {
                format!("Remux {} to FLAC", ctx.lossless_files.len()).into()
            }
            Self::TranscodeLossy => {
                if ctx.lossy_to_flac {
                    format!("Capture {} to FLAC", ctx.lossy_files.len()).into()
                } else {
                    format!(
                        "Transcode {} to Opus ({} kbps)",
                        ctx.lossy_files.len(),
                        ctx.opus_bitrate_kbps
                    )
                    .into()
                }
            }
            Self::ConvertAll => "Convert All".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::RemuxLossless => Color::Green,
            Self::TranscodeLossy => Color::Cyan,
            Self::ConvertAll => Color::Yellow,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::RemuxLossless => ctx.has_lossless(),
            Self::TranscodeLossy => ctx.has_lossy(),
            Self::ConvertAll => ctx.has_lossless() && ctx.has_lossy(),
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> ShitFormatPreviewAction {
        match self {
            Self::RemuxLossless => ShitFormatPreviewAction::ConfirmRemuxLossless,
            Self::TranscodeLossy => ShitFormatPreviewAction::ConfirmTranscodeLossy,
            Self::ConvertAll => ShitFormatPreviewAction::ConfirmConvertAll,
            Self::Cancel => ShitFormatPreviewAction::Cancel,
        }
    }
}
