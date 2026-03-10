//! Corrupt File Resolution Preview UI
//!
//! Shows corrupt files (tag parse errors or waveform decode failures) with
//! action buttons for stash/drop operations.
//!
//! - Up/Down: Scroll file list
//! - Left/Right: Move between action buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::ui::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::{CorruptFileModalData, SelectedButton};
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{
    render_button_row, render_file_path_list, ButtonRects, ConfirmationButton, ListClickTargets,
    PathEntry,
};

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

/// State for the corrupt file resolution modal.
#[derive(Debug)]
pub struct CorruptFilePreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: CorruptFileModalData,
    /// Scroll position for the file list.
    pub scroll: usize,
    /// Which button is selected.
    pub selected_button: SelectedButton,
    /// Click targets for file list items (set during render).
    pub click_targets: ListClickTargets,
    /// Click targets for buttons (set during render).
    pub button_rects: ButtonRects,
}

impl CorruptFilePreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .files
            .get(self.scroll)
            .map(|f| f.corpus_path.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: CorruptFileModalData) -> Self {
        Self {
            cached_data,
            scroll: 0,
            selected_button: SelectedButton::Cancel,
            click_targets: ListClickTargets::new(),
            button_rects: ButtonRects::new(),
        }
    }

    /// Handle a mouse click at (x, y).
    pub fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<CorruptFilePreviewAction> {
        // Check buttons first
        if let Some(button_name) = self.button_rects.hit_test(x, y) {
            match button_name {
                "stash_all" => {
                    self.selected_button = SelectedButton::StashAll;
                    if self.cached_data.has_files() {
                        return Some(CorruptFilePreviewAction::ConfirmStashAll);
                    }
                }
                "cancel" => {
                    self.selected_button = SelectedButton::Cancel;
                    return Some(CorruptFilePreviewAction::Cancel);
                }
                _ => {}
            }
        }
        // Check list items
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.files.len() {
                    self.scroll = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> CorruptFilePreviewAction {
        let has_files = self.cached_data.has_files();

        if crate::ui::helpers::handle_scroll_input(&mut self.scroll, action, self.cached_data.files.len()) {
            return CorruptFilePreviewAction::None;
        }

        match action {
            // Button navigation
            InputAction::NavLeft => {
                self.selected_button.left(has_files);
                CorruptFilePreviewAction::None
            }
            InputAction::NavRight => {
                self.selected_button.right();
                CorruptFilePreviewAction::None
            }

            // Execute selected button
            InputAction::Confirm => match self.selected_button {
                SelectedButton::StashAll if has_files => CorruptFilePreviewAction::ConfirmStashAll,
                SelectedButton::Cancel => CorruptFilePreviewAction::Cancel,
                _ => CorruptFilePreviewAction::None,
            },

            // Cancel
            InputAction::Cancel => CorruptFilePreviewAction::Cancel,

            _ => CorruptFilePreviewAction::None,
        }
    }

    /// Render the corrupt file resolution modal.
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

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Corrupt File Resolution ",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} files)", total),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
    }

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.files.len();

        let block = Block::default()
            .title(format!(" Corrupt Files ({}) ", count))
            .title_style(Style::default().fg(if count > 0 {
                Color::Red
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red));

        let inner = render_pane(f, area, block);

        if self.cached_data.files.is_empty() {
            let empty = Paragraph::new("No corrupt files found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Render description
        let desc_height = 3;
        let desc_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: desc_height,
        };
        let list_area = Rect {
            x: inner.x,
            y: inner.y + desc_height,
            width: inner.width,
            height: inner.height.saturating_sub(desc_height),
        };

        self.click_targets.populate(list_area, self.scroll, self.cached_data.files.len());

        let description = Paragraph::new(vec![
            Line::from("These files have corrupt tags or unreadable audio data."),
            Line::from("Stashing will move them to stash/corrupt/ and drop from index."),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(description, desc_area);

        let entries: Vec<PathEntry> = self
            .cached_data
            .files
            .iter()
            .map(|file| PathEntry::plain(&file.corpus_path))
            .collect();

        render_file_path_list(f, list_area, &entries, self.scroll, self.scroll);
    }

    fn render_controls(&mut self, f: &mut Frame, area: Rect) {
        let has_files = self.cached_data.has_files();

        let block = Block::default().borders(Borders::TOP);
        let inner = render_pane(f, area, block);

        // Track button rects for click detection
        self.button_rects.clear();
        let half = inner.width / 2;
        let left = Rect { width: half, ..inner };
        let right = Rect { x: inner.x + half, width: inner.width - half, ..inner };
        self.button_rects.set("stash_all", left);
        self.button_rects.set("cancel", right);

        let stash_color = if has_files { Color::Red } else { Color::DarkGray };
        let buttons = vec![
            ConfirmationButton::new("Stash & Drop All", stash_color)
                .selected(has_files && self.selected_button == SelectedButton::StashAll),
            ConfirmationButton::new("Cancel", Color::White)
                .selected(self.selected_button == SelectedButton::Cancel),
        ];
        render_button_row(f, inner, &buttons);
    }
}
