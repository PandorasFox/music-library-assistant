//! Missing File resolution — types and preview state.
//!
//! Route: `/resolve/missing-files/restorable`, `/resolve/missing-files/permanent`
//! Query: `GetMissingFileData`

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::health_modals::MissingFileModalData;

use crate::click_targets::ListClickTargets;
use crate::geometry::FocusPane;
use crate::input::InputAction;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrameCore};
use crate::protocol_binding::ProtocolBinding;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingFileAction {
    None,
    ConfirmRestore,
    ConfirmDrop,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingFileButton {
    RestoreAll,
    DropLost,
    #[default]
    Cancel,
}

pub struct MissingFileButtonCtx {
    pub has_restorable: bool,
}

impl ModalButtons for MissingFileButton {
    type Context = MissingFileButtonCtx;
    type Action = MissingFileAction;

    fn all() -> &'static [Self] {
        &[Self::RestoreAll, Self::DropLost, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::RestoreAll => "Restore All".into(),
            Self::DropLost => "Drop Missing".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::RestoreAll if ctx.has_restorable => Color::Green,
            Self::RestoreAll => Color::DarkGray,
            Self::DropLost => Color::Red,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::RestoreAll => ctx.has_restorable,
            Self::DropLost => true,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingFileAction {
        match self {
            Self::RestoreAll => MissingFileAction::ConfirmRestore,
            Self::DropLost => MissingFileAction::ConfirmDrop,
            Self::Cancel => MissingFileAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::RestoreAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::MissingFile,
                label: "Restore missing files".into(),
            },
            Self::DropLost => ProtocolBinding::Transaction {
                decision_key: DecisionKey::MissingFile,
                label: "Drop missing files".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

// ============================================================================
// Preview State
// ============================================================================

/// State for the missing file resolution modal.
///
/// Dual-pane layout: restorable files (left) vs non-restorable (right).
#[derive(Debug)]
pub struct MissingFilePreviewState {
    pub cached_data: MissingFileModalData,
    /// Which list has focus (0 = restorable, 1 = non-restorable).
    pub focused_list: usize,
    /// Cursor position for the restorable list (left pane).
    pub cursor: usize,
    /// Scroll position for the non-restorable list (right pane).
    pub right_scroll: usize,
    /// Click targets for the non-restorable list (right pane).
    pub right_click_targets: ListClickTargets,
    /// Shared frame state (focus, buttons, click targets for left pane).
    pub frame: FrameState<MissingFileButton>,
}

impl MissingFilePreviewState {
    pub fn new(cached_data: MissingFileModalData) -> Self {
        let focused_list = if cached_data.has_restorable() {
            0
        } else if cached_data.has_non_restorable() {
            1
        } else {
            0
        };

        Self {
            cached_data,
            focused_list,
            cursor: 0,
            right_scroll: 0,
            right_click_targets: ListClickTargets::new(),
            frame: FrameState::new(),
        }
    }

    pub fn selected_path(&self) -> Option<&str> {
        if self.focused_list == 0 {
            self.cached_data.restorable.get(self.cursor).map(|f| f.corpus_path.as_str())
        } else {
            self.cached_data.non_restorable.get(self.right_scroll).map(|f| f.corpus_path.as_str())
        }
    }

    pub fn button_ctx(&self) -> MissingFileButtonCtx {
        MissingFileButtonCtx {
            has_restorable: self.cached_data.has_restorable(),
        }
    }

    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<MissingFileAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.restorable.len() {
                    self.frame.focus_pane = FocusPane::List;
                    self.focused_list = 0;
                    self.cursor = idx;
                }
            }
            return None;
        }
        if let Some(id) = self.right_click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.non_restorable.len() {
                    self.frame.focus_pane = FocusPane::List;
                    self.focused_list = 1;
                    self.right_scroll = idx;
                }
            }
        }
        None
    }

    pub fn handle_input(&mut self, action: &InputAction) -> MissingFileAction {
        if self.frame.focus_pane == FocusPane::List {
            match action {
                InputAction::CycleNext | InputAction::CyclePrev => {
                    if self.cached_data.has_restorable() && self.cached_data.has_non_restorable() {
                        self.focused_list = 1 - self.focused_list;
                    }
                    return MissingFileAction::None;
                }
                _ => {}
            }
        }

        if self.frame.focus_pane == FocusPane::List && self.focused_list == 1 {
            match action {
                InputAction::NavUp => {
                    self.right_scroll = self.right_scroll.saturating_sub(1);
                    return MissingFileAction::None;
                }
                InputAction::NavDown => {
                    let max = self.cached_data.non_restorable.len().saturating_sub(1);
                    if self.right_scroll < max { self.right_scroll += 1; }
                    return MissingFileAction::None;
                }
                InputAction::PageUp => {
                    self.right_scroll = self.right_scroll.saturating_sub(10);
                    return MissingFileAction::None;
                }
                InputAction::PageDown => {
                    let max = self.cached_data.non_restorable.len().saturating_sub(1);
                    self.right_scroll = (self.right_scroll + 10).min(max);
                    return MissingFileAction::None;
                }
                _ => {}
            }
        }

        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => MissingFileAction::None,
        }
    }
}

impl ModalFrameCore for MissingFilePreviewState {
    type Button = MissingFileButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit { list_percent: 50, info_height: 3 }
    }

    fn list_title(&self) -> String {
        format!(" Restorable from Library ({}) ", self.cached_data.restorable.len())
    }

    fn empty_message(&self) -> &'static str { "No restorable files" }

    fn frame_state(&self) -> &FrameState<MissingFileButton> { &self.frame }
    fn frame_state_mut(&mut self) -> &mut FrameState<MissingFileButton> { &mut self.frame }
    fn cursor(&self) -> usize { self.cursor }
    fn cursor_mut(&mut self) -> &mut usize { &mut self.cursor }
    fn list_len(&self) -> usize { self.cached_data.restorable.len() }
    fn button_ctx(&self) -> MissingFileButtonCtx { MissingFilePreviewState::button_ctx(self) }
    fn escape_action(&self) -> MissingFileAction { MissingFileAction::Cancel }
}
