//! Shit Format resolution — button and action types only.
//!
//! The shit format modal has a bespoke preview state that doesn't use
//! the generic `ResolutionState`. Only the button enum and action types
//! are lifted here. Will be simplified to lossless-remux-only in future.
//!
//! Route: `/resolve/lossless-remux`
//! Query: `GetShitFormatData`

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;

use crate::modal_buttons::ModalButtons;
use crate::protocol_binding::ProtocolBinding;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShitFormatAction {
    None,
    ConfirmRemuxLossless,
    ConfirmTranscodeLossy,
    ConfirmConvertAll,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ShitFormatButton {
    RemuxLossless,
    TranscodeLossy,
    ConvertAll,
    #[default]
    Cancel,
}

pub struct ShitFormatButtonCtx {
    pub lossless_count: usize,
    pub lossy_count: usize,
    pub has_lossless: bool,
    pub has_lossy: bool,
    pub lossy_to_flac: bool,
    pub opus_bitrate_kbps: u32,
}

impl ModalButtons for ShitFormatButton {
    type Context = ShitFormatButtonCtx;
    type Action = ShitFormatAction;

    fn all() -> &'static [Self] {
        &[Self::RemuxLossless, Self::TranscodeLossy, Self::ConvertAll, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::RemuxLossless => format!("Remux {} to FLAC", ctx.lossless_count).into(),
            Self::TranscodeLossy => {
                if ctx.lossy_to_flac {
                    format!("Capture {} to FLAC", ctx.lossy_count).into()
                } else {
                    format!("Transcode {} to Opus ({} kbps)", ctx.lossy_count, ctx.opus_bitrate_kbps).into()
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
            Self::RemuxLossless => ctx.has_lossless,
            Self::TranscodeLossy => ctx.has_lossy,
            Self::ConvertAll => ctx.has_lossless && ctx.has_lossy,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> ShitFormatAction {
        match self {
            Self::RemuxLossless => ShitFormatAction::ConfirmRemuxLossless,
            Self::TranscodeLossy => ShitFormatAction::ConfirmTranscodeLossy,
            Self::ConvertAll => ShitFormatAction::ConfirmConvertAll,
            Self::Cancel => ShitFormatAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::RemuxLossless => ProtocolBinding::Transaction {
                decision_key: DecisionKey::ShitFormat,
                label: "Remux to FLAC".into(),
            },
            Self::TranscodeLossy => ProtocolBinding::Transaction {
                decision_key: DecisionKey::ShitFormat,
                label: if ctx.lossy_to_flac {
                    "Capture lossy to FLAC".into()
                } else {
                    "Transcode to Opus".into()
                },
            },
            Self::ConvertAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::ShitFormat,
                label: if ctx.lossy_to_flac {
                    "Remux and capture all to FLAC".into()
                } else {
                    "Convert all formats".into()
                },
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
