//! Types for moved file acknowledgement modal.

use std::borrow::Cow;

use ratatui::style::Color;

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;

use mm_meta::views::MovedFileInfo;
use crate::widgets::{FocusPane, ModalButtons};
use crate::widgets::modal_frame::FrameState;

// ============================================================================
// Button Definition
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MovedFileButton {
    #[default]
    Acknowledge,
    Cancel,
}

/// Context for button enablement/labels.
pub struct MovedFileButtonCtx {
    pub has_files: bool,
}

impl ModalButtons for MovedFileButton {
    type Context = MovedFileButtonCtx;
    type Action = MovedFileAction;

    fn all() -> &'static [Self] {
        &[Self::Acknowledge, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Acknowledge => "Acknowledge".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::Acknowledge => Color::Green,
            Self::Cancel => Color::Red,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Acknowledge => ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MovedFileAction {
        match self {
            Self::Acknowledge => MovedFileAction::Acknowledge,
            Self::Cancel => MovedFileAction::Cancel,
        }
    }
}

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovedFileAction {
    None,
    /// Acknowledge all moved files (updates paths in DB, clears signals)
    Acknowledge,
    /// Cancel and return to Insights
    Cancel,
}

// ============================================================================
// State
// ============================================================================

pub struct MovedFileState {
    /// Files with moved signals (cached at modal open)
    pub files: Vec<MovedFileInfo>,
    /// Currently selected file in the list
    pub current_file: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<MovedFileButton>,
}

impl MovedFileState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.files
            .get(self.current_file)
            .map(|f| f.new_path.as_str())
    }

    pub fn new(files: Vec<MovedFileInfo>) -> Self {
        Self {
            files,
            current_file: 0,
            frame: FrameState::new(),
        }
    }

    pub(super) fn button_ctx(&self) -> MovedFileButtonCtx {
        MovedFileButtonCtx {
            has_files: !self.files.is_empty(),
        }
    }

    /// Get data needed for UpdateFilePath mutations.
    ///
    /// Returns (inode, new_path, old_zone, new_zone) tuples for queuing mutations.
    pub fn files_for_mutation(&self) -> Vec<(i64, String, String, String)> {
        self.files
            .iter()
            .map(|f| {
                (
                    f.inode,
                    f.new_path.clone(),
                    f.old_zone.clone(),
                    f.new_zone.clone(),
                )
            })
            .collect()
    }

    /// Handle a mouse click at (x, y). Returns an action if a button was clicked.
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<MovedFileAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.files.len() {
                    self.frame.focus_pane = FocusPane::List;
                    self.current_file = idx;
                }
            }
        }
        None
    }

    pub fn handle_input(&mut self, action: &InputAction) -> MovedFileAction {
        use crate::widgets::modal_frame::{FrameInputResult, ModalFrameCore};
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => MovedFileAction::None,
        }
    }
}
