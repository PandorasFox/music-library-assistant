//! Missing Directory Resolution Preview UI
//!
//! Shows missing directories with action buttons for acknowledgment/drop operations.
//!
//! - Up/Down: Scroll through directory list
//! - Left/Right: Move between action buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::MissingDirectoryModalData;
use crate::helpers::{render_pane, truncate_left};
use crate::widgets::{render_button_row, ButtonRects, ConfirmationButton, ListClickTargets};

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

/// Which button is selected in the controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SelectedButton {
    Drop,
    Cancel,
}

impl SelectedButton {
    pub fn left(&mut self, has_directories: bool) {
        if has_directories {
            *self = SelectedButton::Drop;
        }
    }

    pub fn right(&mut self) {
        *self = SelectedButton::Cancel;
    }
}

/// State for the missing directory resolution modal.
#[derive(Debug)]
pub struct MissingDirectoryPreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: MissingDirectoryModalData,
    /// Scroll position for the directory list.
    pub scroll: usize,
    /// Which button is selected.
    pub selected_button: SelectedButton,
    /// Click targets for directory list items (set during render).
    pub click_targets: ListClickTargets,
    /// Click targets for buttons (set during render).
    pub button_rects: ButtonRects,
}

impl MissingDirectoryPreviewState {
    /// Path of the currently selected directory (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.cached_data
            .directories
            .get(self.scroll)
            .map(|s| s.as_str())
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: MissingDirectoryModalData) -> Self {
        let selected_button = if cached_data.count() > 0 {
            SelectedButton::Drop
        } else {
            SelectedButton::Cancel
        };

        Self {
            cached_data,
            scroll: 0,
            selected_button,
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
    ) -> Option<MissingDirectoryPreviewAction> {
        if let Some(button_name) = self.button_rects.hit_test(x, y) {
            match button_name {
                "drop" => {
                    self.selected_button = SelectedButton::Drop;
                    if self.cached_data.count() > 0 {
                        return Some(MissingDirectoryPreviewAction::ConfirmDrop);
                    }
                }
                "cancel" => {
                    self.selected_button = SelectedButton::Cancel;
                    return Some(MissingDirectoryPreviewAction::Cancel);
                }
                _ => {}
            }
        }
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.count() {
                    self.scroll = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> MissingDirectoryPreviewAction {
        let has_directories = self.cached_data.count() > 0;

        if crate::helpers::handle_scroll_input(&mut self.scroll, action, self.cached_data.count()) {
            return MissingDirectoryPreviewAction::None;
        }

        match action {
            // Button navigation
            InputAction::NavLeft => {
                self.selected_button.left(has_directories);
                MissingDirectoryPreviewAction::None
            }
            InputAction::NavRight => {
                self.selected_button.right();
                MissingDirectoryPreviewAction::None
            }

            // Execute selected button
            InputAction::Confirm => match self.selected_button {
                SelectedButton::Drop if has_directories => {
                    MissingDirectoryPreviewAction::ConfirmDrop
                }
                SelectedButton::Cancel => MissingDirectoryPreviewAction::Cancel,
                _ => MissingDirectoryPreviewAction::None,
            },

            // Cancel
            InputAction::Cancel => MissingDirectoryPreviewAction::Cancel,

            _ => MissingDirectoryPreviewAction::None,
        }
    }

    /// Render the missing directory resolution modal.
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

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.count();

        let border_style = Style::default().fg(Color::Yellow);

        let title = format!(" Deleted Directories ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = render_pane(f, area, block);

        if self.cached_data.directories.is_empty() {
            let empty = Paragraph::new("No missing directories")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Description text
        let desc_area = Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 2,
        };
        let desc = Paragraph::new(
            "These directories were deleted externally. Dropping will remove them and their files from the index."
        )
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(desc, desc_area);

        // List area below description
        let list_area = Rect {
            x: inner.x,
            y: inner.y + 3,
            width: inner.width,
            height: inner.height.saturating_sub(3),
        };

        self.click_targets.populate(list_area, self.scroll, self.cached_data.count());

        let visible_lines = list_area.height as usize;
        let items: Vec<ListItem> = self
            .cached_data
            .directories
            .iter()
            .skip(self.scroll)
            .take(visible_lines)
            .map(|dir| {
                let path = truncate_left(dir, list_area.width.saturating_sub(2) as usize);
                ListItem::new(path).style(Style::default().fg(Color::White))
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, list_area);
    }

    fn render_controls(&mut self, f: &mut Frame, area: Rect) {
        let has_directories = self.cached_data.count() > 0;

        let block = Block::default().borders(Borders::TOP);
        let inner = render_pane(f, area, block);

        // Track button rects for click detection
        self.button_rects.clear();
        let half = inner.width / 2;
        let left = Rect { width: half, ..inner };
        let right = Rect { x: inner.x + half, width: inner.width - half, ..inner };
        self.button_rects.set("drop", left);
        self.button_rects.set("cancel", right);

        let drop_color = if has_directories { Color::Yellow } else { Color::DarkGray };
        let buttons = vec![
            ConfirmationButton::new("Drop All", drop_color)
                .selected(has_directories && self.selected_button == SelectedButton::Drop),
            ConfirmationButton::new("Cancel", Color::White)
                .selected(self.selected_button == SelectedButton::Cancel),
        ];
        render_button_row(f, inner, &buttons);
    }
}
