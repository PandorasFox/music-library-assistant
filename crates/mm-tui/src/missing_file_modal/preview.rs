//! Missing File Resolution Preview UI
//!
//! Shows categorized missing files (restorable vs non-restorable) with
//! action buttons for restore/drop operations.
//!
//! - Shift+Up/Down: Switch focus between list and buttons
//! - Tab: Switch between restorable and non-restorable lists (when list focused)
//! - Up/Down: Scroll within focused list (when list focused)
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

use super::types::{MissingFileButton, MissingFileButtonCtx, MissingFileModalData};
use crate::helpers::{render_pane, truncate_right};
use crate::widgets::{FocusPane, ListClickTargets};
use crate::widgets::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrame};
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};
use crate::widgets::file_path_list::{render_file_path_list, PathEntry};

/// Actions returned from the missing file preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingFilePreviewAction {
    /// No action needed.
    None,
    /// User confirmed restore action - generate HardLink mutations.
    ConfirmRestore,
    /// User confirmed drop action - generate DropFromIndex mutations.
    ConfirmDrop,
    /// Cancel and return to Insights view.
    Cancel,
}

// ============================================================================
// State
// ============================================================================

/// State for the missing file resolution modal.
#[derive(Debug)]
pub struct MissingFilePreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: MissingFileModalData,
    /// Which list has focus (0 = restorable, 1 = non-restorable).
    pub focused_list: usize,
    /// Cursor position for the restorable list (left pane, managed by ModalFrame).
    pub cursor: usize,
    /// Scroll position for the non-restorable list (right pane, managed manually).
    pub right_scroll: usize,
    /// Click targets for the non-restorable list (right pane).
    pub right_click_targets: ListClickTargets,
    /// Shared frame state (focus, buttons, click targets for left pane).
    pub frame: FrameState<MissingFileButton>,
}

impl MissingFilePreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        if self.focused_list == 0 {
            self.cached_data
                .restorable
                .get(self.cursor)
                .map(|f| f.corpus_path.as_str())
        } else {
            self.cached_data
                .non_restorable
                .get(self.right_scroll)
                .map(|f| f.corpus_path.as_str())
        }
    }

    /// Create a new preview state with cached data.
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

    fn button_ctx(&self) -> MissingFileButtonCtx {
        MissingFileButtonCtx {
            has_restorable: self.cached_data.has_restorable(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<MissingFilePreviewAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        // Check left (restorable) list
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
        // Check right (non-restorable) list
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

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> MissingFilePreviewAction {
        // Tab toggles focused list when on List pane
        if self.frame.focus_pane == FocusPane::List {
            match action {
                InputAction::CycleNext | InputAction::CyclePrev => {
                    if self.cached_data.has_restorable() && self.cached_data.has_non_restorable() {
                        self.focused_list = 1 - self.focused_list;
                    }
                    return MissingFilePreviewAction::None;
                }
                _ => {}
            }
        }

        // When focused on right (non-restorable) list, intercept scroll
        if self.frame.focus_pane == FocusPane::List && self.focused_list == 1 {
            match action {
                InputAction::NavUp => {
                    self.right_scroll = self.right_scroll.saturating_sub(1);
                    return MissingFilePreviewAction::None;
                }
                InputAction::NavDown => {
                    let max = self.cached_data.non_restorable.len().saturating_sub(1);
                    if self.right_scroll < max {
                        self.right_scroll += 1;
                    }
                    return MissingFilePreviewAction::None;
                }
                InputAction::PageUp => {
                    self.right_scroll = self.right_scroll.saturating_sub(10);
                    return MissingFilePreviewAction::None;
                }
                InputAction::PageDown => {
                    let max = self.cached_data.non_restorable.len().saturating_sub(1);
                    self.right_scroll = (self.right_scroll + 10).min(max);
                    return MissingFilePreviewAction::None;
                }
                _ => {}
            }
        }

        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => {
                MissingFilePreviewAction::None
            }
        }
    }

    /// Render the missing file resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.render_frame(f, area);
    }
}

// ============================================================================
// ModalFrame Implementation
// ============================================================================

impl ModalFrame for MissingFilePreviewState {
    type Button = MissingFileButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit {
            list_percent: 50,
            info_height: 3,
        }
    }

    fn list_title(&self) -> String {
        format!(
            " Restorable from Library ({}) ",
            self.cached_data.restorable.len()
        )
    }

    fn accent_color(&self) -> Color {
        Color::Cyan
    }

    fn empty_message(&self) -> &'static str {
        "No restorable files"
    }

    fn controls_hints(&self) -> Vec<Span<'static>> {
        let s = Style::default().fg(Color::DarkGray);
        vec![
            Span::styled("Shift+\u{2191}\u{2193}", s),
            Span::styled(" focus  ", s),
            Span::styled("Tab", s),
            Span::styled(" switch list  ", s),
            Span::styled("\u{2190}\u{2192}", s),
            Span::styled(" select  ", s),
            Span::styled("Enter", s),
            Span::styled(" confirm", s),
        ]
    }

    fn frame_state(&self) -> &FrameState<MissingFileButton> {
        &self.frame
    }
    fn frame_state_mut(&mut self) -> &mut FrameState<MissingFileButton> {
        &mut self.frame
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
    fn list_len(&self) -> usize {
        self.cached_data.restorable.len()
    }
    fn button_ctx(&self) -> MissingFileButtonCtx {
        MissingFilePreviewState::button_ctx(self)
    }
    fn escape_action(&self) -> MissingFilePreviewAction {
        MissingFilePreviewAction::Cancel
    }

    fn render_info_bar(&self, f: &mut Frame, area: Rect) {
        let total = self.cached_data.total_count();
        let restorable = self.cached_data.restorable.len();
        let non_restorable = self.cached_data.non_restorable.len();

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Missing File Resolution ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(
                    " ({} total: {} restorable, {} lost)",
                    total, restorable, non_restorable
                ),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));
        f.render_widget(title, area);
    }

    /// Override to apply visual focus tinting based on focused_list.
    fn render_frame_list(&mut self, f: &mut Frame, area: Rect) {
        let count = self.list_len();
        let list_focused =
            self.frame.focus_pane == FocusPane::List && self.focused_list == 0;
        let accent = if list_focused { Color::Green } else { Color::DarkGray };

        let block = Block::default()
            .title(self.list_title())
            .title_style(Style::default().fg(if count > 0 {
                Color::Green
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent));

        let inner = render_pane(f, area, block);
        let cursor = self.cursor;
        self.frame.click_targets.populate(inner, cursor, count);

        if count == 0 {
            f.render_widget(
                Paragraph::new(self.empty_message())
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, cursor, cursor);
    }

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        _is_focused: bool,
    ) -> ListItem<'static> {
        let file = &self.cached_data.restorable[idx];
        let path = truncate_right(&file.corpus_path, width as usize);
        let style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };
        ListItem::new(path).style(style)
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        let detail_focused =
            self.frame.focus_pane == FocusPane::List && self.focused_list == 1;
        let count = self.cached_data.non_restorable.len();
        let accent = if detail_focused { Color::Red } else { Color::DarkGray };

        let title = format!(" Non-Restorable / Data Lost ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 {
                Color::Red
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(accent));

        let inner = render_pane(f, area, block);

        self.right_click_targets.populate(
            inner,
            self.right_scroll,
            count,
        );

        if count == 0 {
            f.render_widget(
                Paragraph::new("No non-restorable files")
                    .style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .non_restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, self.right_scroll, self.right_scroll);
    }
}
