//! Shit Format Resolution Preview UI
//!
//! Shows non-Vorbis container format files with a bitrate slider and
//! action buttons for transcode operations.
//!
//! - Up/Down: Navigate (future: file selection)
//! - Left/Right: Adjust Opus bitrate / button navigation
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph},
    Frame,
};

use super::types::{ShitFormatModalData, SelectedButton};
use crate::ui::helpers::truncate_left;

/// Actions returned from the shit format preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShitFormatPreviewAction {
    /// No action needed.
    None,
    /// User confirmed transcode action.
    ConfirmTranscodeAll,
    /// Cancel and return to Insights view.
    Cancel,
}

/// State for the shit format resolution modal.
#[derive(Debug)]
pub struct ShitFormatPreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: ShitFormatModalData,
    /// Scroll position for the file list.
    pub scroll: usize,
    /// Which button is selected.
    pub selected_button: SelectedButton,
}

impl ShitFormatPreviewState {
    /// Create a new preview state with cached data.
    pub fn new(cached_data: ShitFormatModalData) -> Self {
        Self {
            cached_data,
            scroll: 0,
            selected_button: SelectedButton::Cancel,
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> ShitFormatPreviewAction {
        let has_files = self.cached_data.has_files();

        match key.code {
            // Scroll file list
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ShitFormatPreviewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = self.cached_data.files.len().saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                ShitFormatPreviewAction::None
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                ShitFormatPreviewAction::None
            }
            KeyCode::PageDown => {
                let max = self.cached_data.files.len().saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                ShitFormatPreviewAction::None
            }

            // Bitrate adjustment / button navigation
            KeyCode::Left | KeyCode::Char('h') => {
                // If on TranscodeAll, adjust bitrate down; otherwise navigate
                if self.selected_button == SelectedButton::TranscodeAll {
                    self.cached_data.decrease_bitrate();
                } else {
                    self.selected_button.left(has_files);
                }
                ShitFormatPreviewAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                // If on TranscodeAll, adjust bitrate up; otherwise navigate
                if self.selected_button == SelectedButton::TranscodeAll {
                    self.cached_data.increase_bitrate();
                } else {
                    self.selected_button.right(has_files);
                }
                ShitFormatPreviewAction::None
            }

            // Tab switches between transcode button and cancel
            KeyCode::Tab | KeyCode::BackTab => {
                if has_files {
                    self.selected_button = match self.selected_button {
                        SelectedButton::TranscodeAll => SelectedButton::Cancel,
                        SelectedButton::Cancel => SelectedButton::TranscodeAll,
                    };
                }
                ShitFormatPreviewAction::None
            }

            // Execute selected button
            KeyCode::Enter => match self.selected_button {
                SelectedButton::TranscodeAll if has_files => {
                    ShitFormatPreviewAction::ConfirmTranscodeAll
                }
                SelectedButton::Cancel => ShitFormatPreviewAction::Cancel,
                _ => ShitFormatPreviewAction::None,
            },

            // Cancel
            KeyCode::Esc => ShitFormatPreviewAction::Cancel,

            _ => ShitFormatPreviewAction::None,
        }
    }

    /// Render the shit format resolution modal.
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
                " Shit Format Resolution ",
                Style::default()
                    .fg(Color::Yellow)
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
        // Split into left (bitrate + breakdown) and right (file list)
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        self.render_config_pane(f, chunks[0]);
        self.render_file_list(f, chunks[1]);
    }

    fn render_config_pane(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .title(" Transcode Settings ")
            .title_style(Style::default().fg(Color::Cyan))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));

        let inner = block.inner(area);
        f.render_widget(block, area);

        // Layout: description, bitrate slider, type breakdown
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Description
                Constraint::Length(3), // Bitrate
                Constraint::Min(5),    // Breakdown
            ])
            .split(inner);

        // Description
        let desc = Paragraph::new(vec![
            Line::from("Transcode to Opus for better"),
            Line::from("quality/size and metadata."),
        ])
        .style(Style::default().fg(Color::DarkGray));
        f.render_widget(desc, chunks[0]);

        // Bitrate gauge
        let bitrate = self.cached_data.opus_bitrate_kbps;
        let ratio = (bitrate as f64 - 32.0) / (512.0 - 32.0);
        let bitrate_label = format!("{} kbps", bitrate);
        let gauge = Gauge::default()
            .block(Block::default().title("Opus Bitrate").borders(Borders::NONE))
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
            .ratio(ratio)
            .label(bitrate_label);
        f.render_widget(gauge, chunks[1]);

        // Type breakdown
        let breakdown = self.cached_data.type_breakdown();
        let items: Vec<ListItem> = breakdown
            .iter()
            .map(|(ftype, count)| {
                ListItem::new(format!("  {}: {}", ftype, count))
                    .style(Style::default().fg(Color::White))
            })
            .collect();

        let breakdown_list = List::new(items)
            .block(Block::default().title("File Types").borders(Borders::TOP));
        f.render_widget(breakdown_list, chunks[2]);
    }

    fn render_file_list(&self, f: &mut Frame, area: Rect) {
        let count = self.cached_data.files.len();

        let block = Block::default()
            .title(format!(" Files ({}) ", count))
            .title_style(Style::default().fg(if count > 0 { Color::Yellow } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if self.cached_data.files.is_empty() {
            let empty = Paragraph::new("No shit format files found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Calculate visible lines based on inner area height
        let visible_lines = inner.height as usize;
        let scroll = self.scroll;

        let items: Vec<ListItem> = self
            .cached_data
            .files
            .iter()
            .skip(scroll)
            .take(visible_lines)
            .map(|file| {
                let type_tag = format!("[{}] ", file.file_type);
                let max_path_len = inner.width.saturating_sub(type_tag.len() as u16 + 2) as usize;
                let path = truncate_left(&file.corpus_path, max_path_len);
                ListItem::new(Line::from(vec![
                    Span::styled(type_tag, Style::default().fg(Color::DarkGray)),
                    Span::styled(path, Style::default().fg(Color::White)),
                ]))
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, inner);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let has_files = self.cached_data.has_files();

        // Build button line
        let mut buttons = Vec::new();

        // Transcode All button (with bitrate info)
        let transcode_label = format!(
            " Transcode All to Opus ({} kbps) ",
            self.cached_data.opus_bitrate_kbps
        );
        let transcode_style = if !has_files {
            Style::default().fg(Color::DarkGray)
        } else if self.selected_button == SelectedButton::TranscodeAll {
            Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };
        buttons.push(Span::styled(transcode_label, transcode_style));
        buttons.push(Span::raw("  "));

        // Cancel button
        let cancel_style = if self.selected_button == SelectedButton::Cancel {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        buttons.push(Span::styled(" Cancel ", cancel_style));

        // Hint for bitrate adjustment
        if self.selected_button == SelectedButton::TranscodeAll && has_files {
            buttons.push(Span::raw("  "));
            buttons.push(Span::styled(
                "[</>] adjust bitrate",
                Style::default().fg(Color::DarkGray),
            ));
        }

        let controls = Paragraph::new(Line::from(buttons))
            .block(Block::default().borders(Borders::TOP));

        f.render_widget(controls, area);
    }
}
