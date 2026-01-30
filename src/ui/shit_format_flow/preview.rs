//! Shit Format Resolution Preview UI
//!
//! Shows non-Vorbis container format files split into:
//! - Lossless (WAV, AIFF, APE, WV) → Remux to FLAC
//! - Lossy (MP3, M4A, AAC, WMA) → Transcode to Opus
//!
//! - Up/Down: Navigate file list
//! - Left/Right: Adjust Opus bitrate (when on lossy button) / button navigation
//! - Tab: Cycle between buttons
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
    /// User confirmed remux lossless to FLAC.
    ConfirmRemuxLossless,
    /// User confirmed transcode lossy to Opus.
    ConfirmTranscodeLossy,
    /// User confirmed convert all.
    ConfirmConvertAll,
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
        // Default to first available action
        let default_button = if cached_data.has_lossless() {
            SelectedButton::RemuxLossless
        } else if cached_data.has_lossy() {
            SelectedButton::TranscodeLossy
        } else {
            SelectedButton::Cancel
        };

        Self {
            cached_data,
            scroll: 0,
            selected_button: default_button,
        }
    }

    /// Handle key input.
    pub fn handle_key(&mut self, key: KeyEvent) -> ShitFormatPreviewAction {
        let has_lossless = self.cached_data.has_lossless();
        let has_lossy = self.cached_data.has_lossy();
        let total_files = self.cached_data.total_count();

        match key.code {
            // Scroll file list
            KeyCode::Up | KeyCode::Char('k') => {
                self.scroll = self.scroll.saturating_sub(1);
                ShitFormatPreviewAction::None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let max = total_files.saturating_sub(1);
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
                let max = total_files.saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                ShitFormatPreviewAction::None
            }

            // Bitrate adjustment (only when on lossy buttons)
            KeyCode::Left | KeyCode::Char('h') => {
                if self.selected_button == SelectedButton::TranscodeLossy
                    || self.selected_button == SelectedButton::ConvertAll
                {
                    self.cached_data.decrease_bitrate();
                } else {
                    self.selected_button.prev(has_lossless, has_lossy);
                }
                ShitFormatPreviewAction::None
            }
            KeyCode::Right | KeyCode::Char('l') => {
                if self.selected_button == SelectedButton::TranscodeLossy
                    || self.selected_button == SelectedButton::ConvertAll
                {
                    self.cached_data.increase_bitrate();
                } else {
                    self.selected_button.next(has_lossless, has_lossy);
                }
                ShitFormatPreviewAction::None
            }

            // Tab cycles between buttons
            KeyCode::Tab => {
                self.selected_button.next(has_lossless, has_lossy);
                ShitFormatPreviewAction::None
            }
            KeyCode::BackTab => {
                self.selected_button.prev(has_lossless, has_lossy);
                ShitFormatPreviewAction::None
            }

            // Execute selected button
            KeyCode::Enter => match self.selected_button {
                SelectedButton::RemuxLossless if has_lossless => {
                    ShitFormatPreviewAction::ConfirmRemuxLossless
                }
                SelectedButton::TranscodeLossy if has_lossy => {
                    ShitFormatPreviewAction::ConfirmTranscodeLossy
                }
                SelectedButton::ConvertAll if has_lossless || has_lossy => {
                    ShitFormatPreviewAction::ConfirmConvertAll
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
                Constraint::Length(3), // Controls (extra line for hints)
            ])
            .split(area);

        self.render_title(f, main_chunks[0]);
        self.render_content(f, main_chunks[1]);
        self.render_controls(f, main_chunks[2]);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let lossless = self.cached_data.lossless_files.len();
        let lossy = self.cached_data.lossy_files.len();
        let total = lossless + lossy;

        let title = Paragraph::new(Line::from(vec![
            Span::styled(
                " Shit Format Resolution ",
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                format!(" ({} lossless, {} lossy)", lossless, lossy),
                Style::default().fg(Color::DarkGray),
            ),
        ]))
        .block(Block::default().borders(Borders::ALL));

        f.render_widget(title, area);
    }

    fn render_content(&self, f: &mut Frame, area: Rect) {
        // Split into left (config panes) and right (file list)
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Percentage(40), Constraint::Percentage(60)])
            .split(area);

        self.render_config_pane(f, chunks[0]);
        self.render_file_list(f, chunks[1]);
    }

    fn render_config_pane(&self, f: &mut Frame, area: Rect) {
        let has_lossless = self.cached_data.has_lossless();
        let has_lossy = self.cached_data.has_lossy();

        // Split into lossless and lossy sections
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        // Lossless section
        self.render_lossless_section(f, chunks[0], has_lossless);

        // Lossy section
        self.render_lossy_section(f, chunks[1], has_lossy);
    }

    fn render_lossless_section(&self, f: &mut Frame, area: Rect, has_files: bool) {
        let border_color = if has_files { Color::Green } else { Color::DarkGray };
        let title_color = if has_files { Color::Green } else { Color::DarkGray };

        let block = Block::default()
            .title(" Lossless → FLAC ")
            .title_style(Style::default().fg(title_color))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if !has_files {
            let empty = Paragraph::new("No lossless files")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Layout: description + breakdown
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(2)])
            .split(inner);

        // Description
        let desc = Paragraph::new("Remux to FLAC (lossless)")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(desc, chunks[0]);

        // Type breakdown
        let breakdown = self.cached_data.lossless_breakdown();
        let items: Vec<ListItem> = breakdown
            .iter()
            .map(|(ftype, count)| {
                ListItem::new(format!("  {}: {}", ftype, count))
                    .style(Style::default().fg(Color::White))
            })
            .collect();

        let breakdown_list = List::new(items);
        f.render_widget(breakdown_list, chunks[1]);
    }

    fn render_lossy_section(&self, f: &mut Frame, area: Rect, has_files: bool) {
        let border_color = if has_files { Color::Cyan } else { Color::DarkGray };
        let title_color = if has_files { Color::Cyan } else { Color::DarkGray };

        let block = Block::default()
            .title(" Lossy → Opus ")
            .title_style(Style::default().fg(title_color))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if !has_files {
            let empty = Paragraph::new("No lossy files")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Layout: bitrate slider + breakdown
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(2)])
            .split(inner);

        // Bitrate gauge
        let bitrate = self.cached_data.opus_bitrate_kbps;
        let ratio = (bitrate as f64 - 32.0) / (512.0 - 32.0);
        let bitrate_label = format!("{} kbps", bitrate);
        let gauge = Gauge::default()
            .block(Block::default().title("Opus Bitrate").borders(Borders::NONE))
            .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
            .ratio(ratio)
            .label(bitrate_label);
        f.render_widget(gauge, chunks[0]);

        // Type breakdown
        let breakdown = self.cached_data.lossy_breakdown();
        let items: Vec<ListItem> = breakdown
            .iter()
            .map(|(ftype, count)| {
                ListItem::new(format!("  {}: {}", ftype, count))
                    .style(Style::default().fg(Color::White))
            })
            .collect();

        let breakdown_list = List::new(items);
        f.render_widget(breakdown_list, chunks[1]);
    }

    fn render_file_list(&self, f: &mut Frame, area: Rect) {
        let lossless_count = self.cached_data.lossless_files.len();
        let lossy_count = self.cached_data.lossy_files.len();
        let total = lossless_count + lossy_count;

        let block = Block::default()
            .title(format!(" Files ({}) ", total))
            .title_style(Style::default().fg(if total > 0 { Color::Yellow } else { Color::DarkGray }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner = block.inner(area);
        f.render_widget(block, area);

        if total == 0 {
            let empty = Paragraph::new("No shit format files found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Combine lossless and lossy files for display
        let all_files: Vec<_> = self.cached_data.lossless_files.iter()
            .chain(self.cached_data.lossy_files.iter())
            .collect();

        // Calculate visible lines based on inner area height
        let visible_lines = inner.height as usize;
        let scroll = self.scroll;

        let items: Vec<ListItem> = all_files
            .iter()
            .skip(scroll)
            .take(visible_lines)
            .map(|file| {
                let type_tag = format!("[{}] ", file.file_type);
                let is_lossless = file.is_lossless();
                let tag_color = if is_lossless { Color::Green } else { Color::Cyan };
                let max_path_len = inner.width.saturating_sub(type_tag.len() as u16 + 2) as usize;
                let path = truncate_left(&file.corpus_path, max_path_len);
                ListItem::new(Line::from(vec![
                    Span::styled(type_tag, Style::default().fg(tag_color)),
                    Span::styled(path, Style::default().fg(Color::White)),
                ]))
            })
            .collect();

        let list = List::new(items);
        f.render_widget(list, inner);
    }

    fn render_controls(&self, f: &mut Frame, area: Rect) {
        let has_lossless = self.cached_data.has_lossless();
        let has_lossy = self.cached_data.has_lossy();
        let has_both = has_lossless && has_lossy;

        // Build button line
        let mut buttons = Vec::new();

        // Remux Lossless button
        if has_lossless {
            let label = format!(" Remux {} to FLAC ", self.cached_data.lossless_files.len());
            let style = if self.selected_button == SelectedButton::RemuxLossless {
                Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Green)
            };
            buttons.push(Span::styled(label, style));
            buttons.push(Span::raw(" "));
        }

        // Transcode Lossy button
        if has_lossy {
            let label = format!(
                " Transcode {} to Opus ({} kbps) ",
                self.cached_data.lossy_files.len(),
                self.cached_data.opus_bitrate_kbps
            );
            let style = if self.selected_button == SelectedButton::TranscodeLossy {
                Style::default().fg(Color::Black).bg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Cyan)
            };
            buttons.push(Span::styled(label, style));
            buttons.push(Span::raw(" "));
        }

        // Convert All button (only if both types present)
        if has_both {
            let label = " Convert All ";
            let style = if self.selected_button == SelectedButton::ConvertAll {
                Style::default().fg(Color::Black).bg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            };
            buttons.push(Span::styled(label, style));
            buttons.push(Span::raw(" "));
        }

        // Cancel button
        let cancel_style = if self.selected_button == SelectedButton::Cancel {
            Style::default().fg(Color::Black).bg(Color::White).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        buttons.push(Span::styled(" Cancel ", cancel_style));

        // Hints
        let mut hints = Vec::new();
        let show_bitrate_hint = (self.selected_button == SelectedButton::TranscodeLossy
            || self.selected_button == SelectedButton::ConvertAll) && has_lossy;
        if show_bitrate_hint {
            hints.push(Span::styled(
                " [←/→] adjust bitrate",
                Style::default().fg(Color::DarkGray),
            ));
        }
        hints.push(Span::styled(
            " [Tab] switch buttons",
            Style::default().fg(Color::DarkGray),
        ));

        let controls = Paragraph::new(vec![
            Line::from(buttons),
            Line::from(hints),
        ])
        .block(Block::default().borders(Borders::TOP));

        f.render_widget(controls, area);
    }
}
