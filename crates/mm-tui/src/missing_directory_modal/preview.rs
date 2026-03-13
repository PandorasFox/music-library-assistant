//! Missing Directory Resolution Preview UI
//!
//! Shows missing directories with action buttons for acknowledgment/drop operations.
//!
//! - Shift+Up/Down: Switch focus between list and buttons
//! - Up/Down: Scroll through directory list (when list focused)
//! - Left/Right: Move between action buttons (when buttons focused)
//! - Enter: Execute selected button action
//! - Escape: Cancel

use std::borrow::Cow;

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use super::MissingDirectoryModalData;
use crate::helpers::truncate_left;
use crate::widgets::{FocusPane, ModalButtons};
use crate::widgets::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrame};

/// Actions returned from the missing directory preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingDirectoryPreviewAction {
    /// No action needed.
    None,
    /// User confirmed drop action - generate DropDirectoryFromIndex mutations.
    ConfirmDrop,
    /// Cancel and return to Insights view.
    Cancel,
}

// ============================================================================
// Button Definition
// ============================================================================

/// Button choices for the missing directory resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingDirectoryButton {
    #[default]
    Drop,
    Cancel,
}

/// Lightweight context for button enablement/labels.
pub struct MissingDirectoryButtonCtx {
    pub has_directories: bool,
}

impl ModalButtons for MissingDirectoryButton {
    type Context = MissingDirectoryButtonCtx;
    type Action = MissingDirectoryPreviewAction;

    fn all() -> &'static [Self] {
        &[Self::Drop, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Drop => "Drop All".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Drop if ctx.has_directories => Color::Yellow,
            Self::Drop => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Drop => ctx.has_directories,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingDirectoryPreviewAction {
        match self {
            Self::Drop => MissingDirectoryPreviewAction::ConfirmDrop,
            Self::Cancel => MissingDirectoryPreviewAction::Cancel,
        }
    }
}

// ============================================================================
// State
// ============================================================================

/// State for the missing directory resolution modal.
#[derive(Debug)]
pub struct MissingDirectoryPreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: MissingDirectoryModalData,
    /// Cursor position for the directory list.
    pub cursor: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<MissingDirectoryButton>,
}

impl MissingDirectoryPreviewState {
    /// Path of the currently selected directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .directories
            .get(self.cursor)
            .map(|s| s.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: MissingDirectoryModalData) -> Self {
        let mut frame = FrameState::new();
        if cached_data.count() > 0 {
            frame.buttons.selected = MissingDirectoryButton::Drop;
        }

        Self {
            cached_data,
            cursor: 0,
            frame,
        }
    }

    fn button_ctx(&self) -> MissingDirectoryButtonCtx {
        MissingDirectoryButtonCtx {
            has_directories: self.cached_data.count() > 0,
        }
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<MissingDirectoryPreviewAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.count() {
                    self.frame.focus_pane = FocusPane::List;
                    self.cursor = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> MissingDirectoryPreviewAction {
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => {
                MissingDirectoryPreviewAction::None
            }
        }
    }

    /// Render the missing directory resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.render_frame(f, area);
    }
}

// ============================================================================
// ModalFrame Implementation
// ============================================================================

impl ModalFrame for MissingDirectoryPreviewState {
    type Button = MissingDirectoryButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 2,
        }
    }

    fn list_title(&self) -> String {
        format!(" Deleted Directories ({}) ", self.cached_data.count())
    }

    fn accent_color(&self) -> Color {
        Color::Yellow
    }

    fn empty_message(&self) -> &'static str {
        "No missing directories"
    }

    fn frame_state(&self) -> &FrameState<MissingDirectoryButton> {
        &self.frame
    }
    fn frame_state_mut(&mut self) -> &mut FrameState<MissingDirectoryButton> {
        &mut self.frame
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
    fn list_len(&self) -> usize {
        self.cached_data.count()
    }
    fn button_ctx(&self) -> MissingDirectoryButtonCtx {
        MissingDirectoryPreviewState::button_ctx(self)
    }
    fn escape_action(&self) -> MissingDirectoryPreviewAction {
        MissingDirectoryPreviewAction::Cancel
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.count();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Missing Directory Acknowledgment ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} directories)", count),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, area);
    }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        _is_focused: bool,
    ) -> ListItem<'static> {
        let dir = &self.cached_data.directories[idx];
        let path = truncate_left(dir, width.saturating_sub(2) as usize);
        let style = if is_cursor {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };
        ListItem::new(path).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let desc = Paragraph::new(
            "These directories were deleted externally. \
             Dropping will remove them and their files from the index.",
        )
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(desc, area);
    }
}
