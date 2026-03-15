//! Shit Format resolution — types and preview state.
//!
//! Route: `/resolve/lossless-remux`
//! Query: `GetShitFormatData`

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::cluster_deploy::ShitFormatModalData;

use crate::geometry::FocusPane;
use crate::input::InputAction;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrameCore};
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

// ============================================================================
// Preview State
// ============================================================================

/// State for the shit format resolution modal.
#[derive(Debug)]
pub struct ShitFormatPreviewState {
    pub cached_data: ShitFormatModalData,
    /// Cursor position for the unified file list.
    pub cursor: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<ShitFormatButton>,
}

impl ShitFormatPreviewState {
    pub fn new(cached_data: ShitFormatModalData) -> Self {
        let mut frame = FrameState::new();
        if cached_data.has_lossless() {
            frame.buttons.selected = ShitFormatButton::RemuxLossless;
        } else if cached_data.has_lossy() {
            frame.buttons.selected = ShitFormatButton::TranscodeLossy;
        }

        Self {
            cached_data,
            cursor: 0,
            frame,
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        let lossless_len = self.cached_data.lossless_files.len();
        if self.cursor < lossless_len {
            self.cached_data.lossless_files.get(self.cursor).map(|f| f.corpus_path.as_str())
        } else {
            self.cached_data.lossy_files.get(self.cursor - lossless_len).map(|f| f.corpus_path.as_str())
        }
    }

    pub fn button_ctx(&self) -> ShitFormatButtonCtx {
        ShitFormatButtonCtx {
            lossless_count: self.cached_data.lossless_files.len(),
            lossy_count: self.cached_data.lossy_files.len(),
            has_lossless: self.cached_data.has_lossless(),
            has_lossy: self.cached_data.has_lossy(),
            lossy_to_flac: self.cached_data.lossy_to_flac,
            opus_bitrate_kbps: self.cached_data.opus_bitrate_kbps,
        }
    }

    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<ShitFormatAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.total_count() {
                    self.frame.focus_pane = FocusPane::List;
                    self.cursor = idx;
                }
            }
        }
        None
    }

    pub fn handle_input(&mut self, action: &InputAction) -> ShitFormatAction {
        // Tab cycles between buttons (works regardless of focus pane)
        match action {
            InputAction::CycleNext => {
                let ctx = self.button_ctx();
                self.frame.buttons.nav_right(&ctx);
                return ShitFormatAction::None;
            }
            InputAction::CyclePrev => {
                let ctx = self.button_ctx();
                self.frame.buttons.nav_left(&ctx);
                return ShitFormatAction::None;
            }
            _ => {}
        }

        // NavLeft/NavRight: bitrate adjustment when on lossy button in Buttons pane
        if self.frame.focus_pane == FocusPane::Buttons {
            let on_lossy = matches!(
                self.frame.buttons.selected,
                ShitFormatButton::TranscodeLossy | ShitFormatButton::ConvertAll
            );
            if on_lossy && !self.cached_data.lossy_to_flac {
                match action {
                    InputAction::NavLeft => {
                        self.cached_data.decrease_bitrate();
                        return ShitFormatAction::None;
                    }
                    InputAction::NavRight => {
                        self.cached_data.increase_bitrate();
                        return ShitFormatAction::None;
                    }
                    _ => {}
                }
            }
        }

        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => ShitFormatAction::None,
        }
    }
}

impl ModalFrameCore for ShitFormatPreviewState {
    type Button = ShitFormatButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit { list_percent: 60, info_height: 3 }
    }

    fn list_title(&self) -> String {
        format!(" Files ({}) ", self.cached_data.total_count())
    }

    fn empty_message(&self) -> &'static str { "No shit format files found" }

    fn frame_state(&self) -> &FrameState<ShitFormatButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<ShitFormatButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.cursor }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.cursor }
    fn list_len(&self) -> usize { self.cached_data.total_count() }
    fn button_ctx(&self) -> ShitFormatButtonCtx { ShitFormatPreviewState::button_ctx(self) }
    fn escape_action(&self) -> ShitFormatAction { ShitFormatAction::Cancel }
}
