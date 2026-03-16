//! Lossless Remux resolution — types and preview state.
//!
//! Route: `/resolve/lossless-remux`
//! Query: `GetLosslessRemuxData`

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::cluster_deploy::LosslessRemuxModalData;

use crate::geometry::FocusPane;
use crate::input::InputAction;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrameCore};
use crate::protocol_binding::ProtocolBinding;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LosslessRemuxAction {
    None,
    Confirm,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LosslessRemuxButton {
    Remux,
    #[default]
    Cancel,
}

pub struct LosslessRemuxButtonCtx {
    pub count: usize,
    pub has_files: bool,
}

impl ModalButtons for LosslessRemuxButton {
    type Context = LosslessRemuxButtonCtx;
    type Action = LosslessRemuxAction;

    fn all() -> &'static [Self] {
        &[Self::Remux, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Remux => format!("Remux {} to FLAC", ctx.count).into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::Remux => Color::Green,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Remux => ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> LosslessRemuxAction {
        match self {
            Self::Remux => LosslessRemuxAction::Confirm,
            Self::Cancel => LosslessRemuxAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Remux => ProtocolBinding::Transaction {
                decision_key: DecisionKey::LosslessRemux,
                label: "Remux to FLAC".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

// ============================================================================
// Preview State
// ============================================================================

/// State for the lossless remux resolution modal.
#[derive(Debug)]
pub struct LosslessRemuxPreviewState {
    pub cached_data: LosslessRemuxModalData,
    /// Cursor position for the file list.
    pub cursor: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<LosslessRemuxButton>,
}

impl LosslessRemuxPreviewState {
    pub fn new(cached_data: LosslessRemuxModalData) -> Self {
        let mut frame = FrameState::new();
        if cached_data.has_files() {
            frame.buttons.selected = LosslessRemuxButton::Remux;
        }

        Self {
            cached_data,
            cursor: 0,
            frame,
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data.files.get(self.cursor).map(|f| f.corpus_path.as_str())
    }

    pub fn button_ctx(&self) -> LosslessRemuxButtonCtx {
        LosslessRemuxButtonCtx {
            count: self.cached_data.total_count(),
            has_files: self.cached_data.has_files(),
        }
    }

    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<LosslessRemuxAction> {
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

    pub fn handle_input(&mut self, action: &InputAction) -> LosslessRemuxAction {
        // Tab cycles between buttons (works regardless of focus pane)
        match action {
            InputAction::CycleNext => {
                let ctx = self.button_ctx();
                self.frame.buttons.nav_right(&ctx);
                return LosslessRemuxAction::None;
            }
            InputAction::CyclePrev => {
                let ctx = self.button_ctx();
                self.frame.buttons.nav_left(&ctx);
                return LosslessRemuxAction::None;
            }
            _ => {}
        }

        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => LosslessRemuxAction::None,
        }
    }
}

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for LosslessRemuxPreviewState {
    type Action = LosslessRemuxAction;

    fn dispatch(
        &self,
        action: LosslessRemuxAction,
        resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            LosslessRemuxAction::None => DispatchResult::Handled,
            LosslessRemuxAction::Confirm => {
                let mutations = self.cached_data.mutations(resolver);
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                DispatchResult::Stage {
                    key: DecisionKey::LosslessRemux,
                    label: "Remux to FLAC".into(),
                    mutations,
                }
            }
            LosslessRemuxAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Lossless remux cancelled"
    }
}

impl ModalFrameCore for LosslessRemuxPreviewState {
    type Button = LosslessRemuxButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit { list_percent: 60, info_height: 3 }
    }

    fn list_title(&self) -> String {
        format!(" Files ({}) ", self.cached_data.total_count())
    }

    fn empty_message(&self) -> &'static str { "No lossless remux candidates found" }

    fn frame_state(&self) -> &FrameState<LosslessRemuxButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<LosslessRemuxButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.cursor }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.cursor }
    fn list_len(&self) -> usize { self.cached_data.total_count() }
    fn button_ctx(&self) -> LosslessRemuxButtonCtx { LosslessRemuxPreviewState::button_ctx(self) }
    fn escape_action(&self) -> LosslessRemuxAction { LosslessRemuxAction::Cancel }
}
