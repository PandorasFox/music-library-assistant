//! Corrupt File Resolution Preview UI
//!
//! Shows corrupt files (tag parse errors or waveform decode failures) with
//! action buttons for stash/drop operations.
//!
//! - Shift+Up/Down: Switch focus between list and buttons
//! - Up/Down: Scroll file list (when list focused)
//! - Left/Right: Move between action buttons (when buttons focused)
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, ListItem, Paragraph},
    Frame,
};

use super::types::{CorruptButton, CorruptButtonCtx, CorruptFileModalData};
use crate::helpers::truncate_right;
use crate::widgets::FocusPane;
use crate::widgets::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrame, ModalFrameCore};
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

/// Actions returned from the corrupt file preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorruptFilePreviewAction {
    /// No action needed.
    None,
    /// User confirmed stash + drop action.
    ConfirmStashAll,
    /// Cancel and return to Insights view.
    Cancel,
}

// ============================================================================
// State
// ============================================================================

/// State for the corrupt file resolution modal.
#[derive(Debug)]
pub struct CorruptFilePreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: CorruptFileModalData,
    /// Cursor position for the file list.
    pub cursor: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<CorruptButton>,
}

impl CorruptFilePreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .files
            .get(self.cursor)
            .map(|f| f.corpus_path.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: CorruptFileModalData) -> Self {
        Self {
            cached_data,
            cursor: 0,
            frame: FrameState::new(),
        }
    }

    fn button_ctx(&self) -> CorruptButtonCtx {
        CorruptButtonCtx {
            has_files: self.cached_data.has_files(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<CorruptFilePreviewAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.files.len() {
                    self.frame.focus_pane = FocusPane::List;
                    self.cursor = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> CorruptFilePreviewAction {
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => {
                CorruptFilePreviewAction::None
            }
        }
    }

    /// Render the corrupt file resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.render_frame(f, area);
    }
}

// ============================================================================
// ModalFrame Implementation
// ============================================================================

impl ModalFrameCore for CorruptFilePreviewState {
    type Button = CorruptButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 3,
        }
    }

    fn list_title(&self) -> String {
        format!(" Corrupt Files ({}) ", self.cached_data.files.len())
    }

    fn empty_message(&self) -> &'static str {
        "No corrupt files found"
    }

    fn frame_state(&self) -> &FrameState<CorruptButton> {
        &self.frame
    }
    fn frame_state_mut(&mut self) -> &mut FrameState<CorruptButton> {
        &mut self.frame
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
    fn list_len(&self) -> usize {
        self.cached_data.files.len()
    }
    fn button_ctx(&self) -> CorruptButtonCtx {
        CorruptFilePreviewState::button_ctx(self)
    }
    fn escape_action(&self) -> CorruptFilePreviewAction {
        CorruptFilePreviewAction::Cancel
    }
}

impl ModalFrame for CorruptFilePreviewState {
    fn accent_color(&self) -> Color {
        Color::Red
    }

    fn render_header(&self, f: &mut Frame, area: Rect) {
        let total = self.cached_data.total_count();
        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Corrupt File Resolution ",
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} files)", total),
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
        let file = &self.cached_data.files[idx];
        let path = truncate_right(&file.corpus_path, width as usize);
        let style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };
        ListItem::new(path).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let description = Paragraph::new(vec![
            Line::from("These files have corrupt tags or unreadable audio data."),
            Line::from("Stashing will move them to stash/corrupt/ and drop from index."),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(description, area);
    }
}
