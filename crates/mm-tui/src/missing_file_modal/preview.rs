//! Missing File Resolution Preview UI
//!
//! Shows categorized missing files (restorable vs non-restorable) with
//! action buttons for restore/drop operations.
//!
//! - Tab: Switch between lists
//! - Up/Down: Scroll within focused list
//! - Left/Right: Move between action buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::{MissingFileModalData, SelectedButton};
use crate::helpers::render_pane;
use crate::widgets::{
    render_button_row, render_file_path_list, ButtonRects, ConfirmationButton, ListClickTargets,
    PathEntry,
};

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

/// State for the missing file resolution modal.
#[derive(Debug)]
pub struct MissingFilePreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: MissingFileModalData,
    /// Which list has focus (0 = restorable, 1 = non-restorable).
    pub focused_list: usize,
    /// Scroll position for each list.
    pub scroll: [usize; 2],
    /// Which button is selected.
    pub selected_button: SelectedButton,
    /// Click targets for restorable list (set during render).
    pub click_targets_restorable: ListClickTargets,
    /// Click targets for non-restorable list (set during render).
    pub click_targets_non_restorable: ListClickTargets,
    /// Click targets for buttons (set during render).
    pub button_rects: ButtonRects,
}

impl MissingFilePreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        let idx = self.scroll[self.focused_list];
        if self.focused_list == 0 {
            self.cached_data
                .restorable
                .get(idx)
                .map(|f| f.corpus_path.as_str())
        } else {
            self.cached_data
                .non_restorable
                .get(idx)
                .map(|f| f.corpus_path.as_str())
        }
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: MissingFileModalData) -> Self {
        // Focus the list that has items
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
            scroll: [0, 0],
            selected_button: SelectedButton::Cancel,
            click_targets_restorable: ListClickTargets::new(),
            click_targets_non_restorable: ListClickTargets::new(),
            button_rects: ButtonRects::new(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<MissingFilePreviewAction> {
        let has_restorable = self.cached_data.has_restorable();

        // Check buttons first
        if let Some(button_name) = self.button_rects.hit_test(x, y) {
            match button_name {
                "restore_all" => {
                    self.selected_button = SelectedButton::RestoreAll;
                    if has_restorable {
                        return Some(MissingFilePreviewAction::ConfirmRestore);
                    }
                }
                "drop_lost" => {
                    self.selected_button = SelectedButton::DropLost;
                    return Some(MissingFilePreviewAction::ConfirmDrop);
                }
                "cancel" => {
                    self.selected_button = SelectedButton::Cancel;
                    return Some(MissingFilePreviewAction::Cancel);
                }
                _ => {}
            }
        }
        // Check restorable list
        if let Some(id) = self.click_targets_restorable.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.restorable.len() {
                    self.focused_list = 0;
                    self.scroll[0] = idx;
                }
            }
            return None;
        }
        // Check non-restorable list
        if let Some(id) = self.click_targets_non_restorable.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.non_restorable.len() {
                    self.focused_list = 1;
                    self.scroll[1] = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> MissingFilePreviewAction {
        let has_restorable = self.cached_data.has_restorable();
        let has_non_restorable = self.cached_data.has_non_restorable();

        // Scroll within focused list
        let count = self.list_count(self.focused_list);
        if crate::helpers::handle_scroll_input(
            &mut self.scroll[self.focused_list],
            action,
            count,
        ) {
            return MissingFilePreviewAction::None;
        }

        match action {
            // Switch between lists
            InputAction::CycleNext | InputAction::CyclePrev => {
                if has_restorable && has_non_restorable {
                    self.focused_list = 1 - self.focused_list;
                }
                MissingFilePreviewAction::None
            }

            // Button navigation
            InputAction::NavLeft => {
                self.selected_button.left(has_restorable);
                MissingFilePreviewAction::None
            }
            InputAction::NavRight => {
                self.selected_button.right(has_restorable);
                MissingFilePreviewAction::None
            }

            // Execute selected button
            InputAction::Confirm => match self.selected_button {
                SelectedButton::RestoreAll if has_restorable => {
                    MissingFilePreviewAction::ConfirmRestore
                }
                SelectedButton::DropLost => MissingFilePreviewAction::ConfirmDrop,
                SelectedButton::Cancel => MissingFilePreviewAction::Cancel,
                _ => MissingFilePreviewAction::None,
            },

            // Cancel
            InputAction::Cancel => MissingFilePreviewAction::Cancel,

            _ => MissingFilePreviewAction::None,
        }
    }

    fn list_count(&self, list_idx: usize) -> usize {
        if list_idx == 0 {
            self.cached_data.restorable.len()
        } else {
            self.cached_data.non_restorable.len()
        }
    }

    /// Render the missing file resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Clear background
        f.render_widget(Clear, area);

        // Layout: title + content + controls
        let main_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Title
                Constraint::Min(10),   // Content
                Constraint::Length(2), // Controls
            ])
            .split(area);

        self.render_title(f, main_chunks[0]);
        self.render_content(f, main_chunks[1]);
        self.render_controls(f, main_chunks[2]);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
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

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        // Split into two panes for the two lists
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        self.render_restorable_list(f, chunks[0]);
        self.render_non_restorable_list(f, chunks[1]);
    }

    fn render_restorable_list(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focused_list == 0;
        let count = self.cached_data.restorable.len();

        let border_style = if focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = format!(" Restorable from Library ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 {
                Color::Green
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = render_pane(f, area, block);

        self.click_targets_restorable.populate(inner, self.scroll[0], self.cached_data.restorable.len());

        if self.cached_data.restorable.is_empty() {
            let empty =
                Paragraph::new("No restorable files").style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, self.scroll[0], self.scroll[0]);
    }

    fn render_non_restorable_list(&mut self, f: &mut Frame, area: Rect) {
        let focused = self.focused_list == 1;
        let count = self.cached_data.non_restorable.len();

        let border_style = if focused {
            Style::default().fg(Color::Red)
        } else {
            Style::default().fg(Color::DarkGray)
        };

        let title = format!(" Non-Restorable / Data Lost ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 {
                Color::Red
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = render_pane(f, area, block);

        self.click_targets_non_restorable.populate(inner, self.scroll[1], self.cached_data.non_restorable.len());

        if self.cached_data.non_restorable.is_empty() {
            let empty = Paragraph::new("No non-restorable files")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        let entries: Vec<PathEntry> = self
            .cached_data
            .non_restorable
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, inner, &entries, self.scroll[1], self.scroll[1]);
    }

    fn render_controls(&mut self, f: &mut Frame, area: Rect) {
        let has_restorable = self.cached_data.has_restorable();

        let block = Block::default().borders(Borders::TOP);
        let inner = render_pane(f, area, block);

        // Track button rects for click detection
        let third = inner.width / 3;
        self.button_rects.clear();
        self.button_rects.set("restore_all", Rect { width: third, ..inner });
        self.button_rects.set("drop_lost", Rect { x: inner.x + third, width: third, ..inner });
        self.button_rects.set("cancel", Rect { x: inner.x + 2 * third, width: inner.width - 2 * third, ..inner });

        let restore_color = if has_restorable { Color::Green } else { Color::DarkGray };
        let buttons = vec![
            ConfirmationButton::new("Restore All", restore_color)
                .selected(has_restorable && self.selected_button == SelectedButton::RestoreAll),
            ConfirmationButton::new("Drop Missing", Color::Red)
                .selected(self.selected_button == SelectedButton::DropLost),
            ConfirmationButton::new("Cancel", Color::White)
                .selected(self.selected_button == SelectedButton::Cancel),
        ];
        render_button_row(f, inner, &buttons);
    }
}
