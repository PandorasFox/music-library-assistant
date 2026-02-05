//! Missing Directory Resolution Preview UI
//!
//! Shows missing directories with action buttons for acknowledgment/drop operations.
//!
//! - Up/Down: Scroll through directory list
//! - Left/Right: Move between action buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::MissingDirectoryModalData;
use crate::ui::helpers::truncate_left;

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
}

impl MissingDirectoryPreviewState {
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
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> MissingDirectoryPreviewAction {
        let has_directories = self.cached_data.count() > 0;

        match key.code {
            // Scroll within list
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
                MissingDirectoryPreviewAction::None
            }
            KeyCode::Down => {
                let max = self.cached_data.count().saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                MissingDirectoryPreviewAction::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                MissingDirectoryPreviewAction::None
            }
            KeyCode::PageDown => {
                let max = self.cached_data.count().saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                MissingDirectoryPreviewAction::None
            }

            // Button navigation
            KeyCode::Left => {
                self.selected_button.left(has_directories);
                MissingDirectoryPreviewAction::None
            }
            KeyCode::Right => {
                self.selected_button.right();
                MissingDirectoryPreviewAction::None
            }

            // Execute selected button
            KeyCode::Enter => match self.selected_button {
                SelectedButton::Drop if has_directories => {
                    MissingDirectoryPreviewAction::ConfirmDrop
                }
                SelectedButton::Cancel => MissingDirectoryPreviewAction::Cancel,
                _ => MissingDirectoryPreviewAction::None,
            },

            // Cancel
            KeyCode::Esc => MissingDirectoryPreviewAction::Cancel,

            _ => MissingDirectoryPreviewAction::None,
        }
    }

    /// Render the missing directory resolution modal.
    pub fn render(&self, f: &mut Frame, area: Rect) {
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

    fn render_content(&self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.count();

        let border_style = Style::default().fg(Color::Yellow);

        let title = format!(" Deleted Directories ({}) ", count);
        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(if count > 0 { Color::Yellow } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(border_style);

        let inner = block.inner(area);
        f.render_widget(block, area);

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

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let has_directories = self.cached_data.count() > 0;

        // Build button line
        let mut buttons = Vec::new();

        // Drop button
        let drop_style = if !has_directories {
            Style::default().fg(Color::DarkGray)
        } else if self.selected_button == SelectedButton::Drop {
            Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Yellow)
        };
        buttons.push(Span::styled(" Drop All ", drop_style));
        buttons.push(Span::raw("  "));

        // Cancel button
        let cancel_style = if self.selected_button == SelectedButton::Cancel {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        buttons.push(Span::styled(" Cancel ", cancel_style));

        let controls = Paragraph::new(Line::from(buttons))
            .block(Block::default().borders(Borders::TOP));

        f.render_widget(controls, area);
    }
}
