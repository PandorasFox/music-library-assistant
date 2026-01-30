//! Corrupt File Resolution Preview UI
//!
//! Shows corrupt files (tag parse errors or waveform decode failures) with
//! action buttons for stash/drop operations.
//!
//! - Up/Down: Scroll file list
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

use super::types::{CorruptFileModalData, SelectedButton};
use crate::ui::helpers::truncate_left;

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
}

impl CorruptFilePreviewState {
    /// Create a new preview state with cached data.
    pub fn new(cached_data: CorruptFileModalData) -> Self {
        Self {
            cached_data,
            scroll: 0,
            selected_button: SelectedButton::Cancel,
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> CorruptFilePreviewAction {
        let has_files = self.cached_data.has_files();

        match key.code {
            // Scroll file list
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                CorruptFilePreviewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.cached_data.files.len().saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                CorruptFilePreviewAction::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                CorruptFilePreviewAction::None
            }
            KeyCode::PageDown => {
                let max = self.cached_data.files.len().saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                CorruptFilePreviewAction::None
            }

            // Button navigation
            KeyCode::Left | KeyCode::Char('h') => {
                self.selected_button.left(has_files);
                CorruptFilePreviewAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.selected_button.right(has_files);
                CorruptFilePreviewAction::None
            }

            // Execute selected button
            KeyCode::Enter => match self.selected_button {
                SelectedButton::StashAll if has_files => {
                    CorruptFilePreviewAction::ConfirmStashAll
                }
                SelectedButton::Cancel => CorruptFilePreviewAction::Cancel,
                _ => CorruptFilePreviewAction::None,
            },

            // Cancel
            KeyCode::Esc => CorruptFilePreviewAction::Cancel,

            _ => CorruptFilePreviewAction::None,
        }
    }

    /// Render the corrupt file resolution modal.
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

    fn render_content(&self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.files.len();

        let block = Block::default()
            .title(format!(" Corrupt Files ({}) ", count))
            .title_style(Style::default().fg(if count > 0 { Color::Red } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Red));

        let inner = block.inner(area);
        f.render_widget(block, area);

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

        let description = Paragraph::new(vec![
            Line::from("These files have corrupt tags or unreadable audio data."),
            Line::from("Stashing will move them to stash/corrupt/ and drop from index."),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(description, desc_area);

        // Calculate visible lines based on list area height
        let visible_lines = list_area.height as usize;
        let scroll = self.scroll;

        let items: Vec<ListItem> = self
            .cached_data
            .files
            .iter()
            .skip(scroll)
            .take(visible_lines)
            .map(|file| {
                let path = truncate_left(&file.corpus_path, list_area.width.saturating_sub(2) as usize);
                ListItem::new(path).style(Style::default().fg(Color::White))
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, list_area);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let has_files = self.cached_data.has_files();

        // Build button line
        let mut buttons = Vec::new();

        // Stash All button
        let stash_style = if !has_files {
            Style::default().fg(Color::DarkGray)
        } else if self.selected_button == SelectedButton::StashAll {
            Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red)
        };
        buttons.push(Span::styled(" Stash & Drop All ", stash_style));
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
