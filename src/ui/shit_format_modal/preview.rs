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

use crate::ui::action_handlers::witness::ConfirmationGesture;
use crate::ui::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, List, ListItem, Paragraph},
    Frame,
};

use super::types::{SelectedButton, ShitFormatModalData};
use crate::ui::helpers::render_pane;
use crate::ui::widgets::{render_file_path_list, ButtonRects, ListClickTargets, PathEntry};

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
    /// Click targets for file list items (set during render).
    pub click_targets: ListClickTargets,
    /// Click targets for buttons (set during render).
    pub button_rects: ButtonRects,
}

impl ShitFormatPreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        let lossless_len = self.cached_data.lossless_files.len();
        if self.scroll < lossless_len {
            self.cached_data
                .lossless_files
                .get(self.scroll)
                .map(|f| f.corpus_path.as_str())
        } else {
            self.cached_data
                .lossy_files
                .get(self.scroll - lossless_len)
                .map(|f| f.corpus_path.as_str())
        }
    }

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
    ) -> Option<ShitFormatPreviewAction> {
        let has_lossless = self.cached_data.has_lossless();
        let has_lossy = self.cached_data.has_lossy();

        // Check buttons first
        if let Some(button_name) = self.button_rects.hit_test(x, y) {
            match button_name {
                "remux_lossless" => {
                    self.selected_button = SelectedButton::RemuxLossless;
                    if has_lossless {
                        return Some(ShitFormatPreviewAction::ConfirmRemuxLossless);
                    }
                }
                "transcode_lossy" => {
                    self.selected_button = SelectedButton::TranscodeLossy;
                    if has_lossy {
                        return Some(ShitFormatPreviewAction::ConfirmTranscodeLossy);
                    }
                }
                "convert_all" => {
                    self.selected_button = SelectedButton::ConvertAll;
                    if has_lossless || has_lossy {
                        return Some(ShitFormatPreviewAction::ConfirmConvertAll);
                    }
                }
                "cancel" => {
                    self.selected_button = SelectedButton::Cancel;
                    return Some(ShitFormatPreviewAction::Cancel);
                }
                _ => {}
            }
        }
        // Check list items
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.total_count() {
                    self.scroll = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> ShitFormatPreviewAction {
        let has_lossless = self.cached_data.has_lossless();
        let has_lossy = self.cached_data.has_lossy();
        let total_files = self.cached_data.total_count();

        match action {
            // Scroll file list
            InputAction::NavUp => {
                self.scroll = self.scroll.saturating_sub(1);
                ShitFormatPreviewAction::None
            }
            InputAction::NavDown => {
                let max = total_files.saturating_sub(1);
                if self.scroll < max {
                    self.scroll += 1;
                }
                ShitFormatPreviewAction::None
            }
            InputAction::PageUp => {
                self.scroll = self.scroll.saturating_sub(10);
                ShitFormatPreviewAction::None
            }
            InputAction::PageDown => {
                let max = total_files.saturating_sub(1);
                self.scroll = (self.scroll + 10).min(max);
                ShitFormatPreviewAction::None
            }

            // Bitrate adjustment (only when on lossy buttons, and not in FLAC capture mode)
            InputAction::NavLeft => {
                let on_lossy_button = self.selected_button == SelectedButton::TranscodeLossy
                    || self.selected_button == SelectedButton::ConvertAll;
                if on_lossy_button && !self.cached_data.lossy_to_flac {
                    self.cached_data.decrease_bitrate();
                } else {
                    self.selected_button.prev(has_lossless, has_lossy);
                }
                ShitFormatPreviewAction::None
            }
            InputAction::NavRight => {
                let on_lossy_button = self.selected_button == SelectedButton::TranscodeLossy
                    || self.selected_button == SelectedButton::ConvertAll;
                if on_lossy_button && !self.cached_data.lossy_to_flac {
                    self.cached_data.increase_bitrate();
                } else {
                    self.selected_button.next(has_lossless, has_lossy);
                }
                ShitFormatPreviewAction::None
            }

            // Tab cycles between buttons
            InputAction::CycleNext => {
                self.selected_button.next(has_lossless, has_lossy);
                ShitFormatPreviewAction::None
            }
            InputAction::CyclePrev => {
                self.selected_button.prev(has_lossless, has_lossy);
                ShitFormatPreviewAction::None
            }

            // Execute selected button
            InputAction::Confirm => match self.selected_button {
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
            InputAction::Cancel => ShitFormatPreviewAction::Cancel,

            _ => ShitFormatPreviewAction::None,
        }
    }

    /// Render the shit format resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
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

    fn render_content(&mut self, f: &mut Frame, area: Rect) {
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
        let border_color = if has_files {
            Color::Green
        } else {
            Color::DarkGray
        };
        let title_color = if has_files {
            Color::Green
        } else {
            Color::DarkGray
        };

        let block = Block::default()
            .title(" Lossless → FLAC ")
            .title_style(Style::default().fg(title_color))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = render_pane(f, area, block);

        if !has_files {
            let empty =
                Paragraph::new("No lossless files").style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Layout: description + breakdown
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(2)])
            .split(inner);

        // Description
        let desc =
            Paragraph::new("Remux to FLAC (lossless)").style(Style::default().fg(Color::DarkGray));
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
        let lossy_to_flac = self.cached_data.lossy_to_flac;
        let border_color = if has_files {
            Color::Cyan
        } else {
            Color::DarkGray
        };
        let title_color = if has_files {
            Color::Cyan
        } else {
            Color::DarkGray
        };

        let title = if lossy_to_flac {
            " Lossy \u{2192} FLAC (lossy capture) "
        } else {
            " Lossy \u{2192} Opus "
        };

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(title_color))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(border_color));

        let inner = render_pane(f, area, block);

        if !has_files {
            let empty =
                Paragraph::new("No lossy files").style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        if lossy_to_flac {
            // FLAC lossy capture mode: description + breakdown (no bitrate gauge)
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(2)])
                .split(inner);

            let desc = Paragraph::new("Capture decoded waveform to FLAC")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(desc, chunks[0]);

            let breakdown = self.cached_data.lossy_breakdown();
            let items: Vec<ListItem> = breakdown
                .iter()
                .map(|(ftype, count)| {
                    ListItem::new(format!("  {}: {}", ftype, count))
                        .style(Style::default().fg(Color::White))
                })
                .collect();
            f.render_widget(List::new(items), chunks[1]);
        } else {
            // Opus mode: bitrate slider + breakdown
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(2)])
                .split(inner);

            // Bitrate gauge
            let bitrate = self.cached_data.opus_bitrate_kbps;
            let ratio = (bitrate as f64 - 32.0) / (512.0 - 32.0);
            let bitrate_label = format!("{} kbps", bitrate);
            let gauge = Gauge::default()
                .block(
                    Block::default()
                        .title("Opus Bitrate")
                        .borders(Borders::NONE),
                )
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
            f.render_widget(List::new(items), chunks[1]);
        }
    }

    fn render_file_list(&mut self, f: &mut Frame, area: Rect) {
        let lossless_count = self.cached_data.lossless_files.len();
        let lossy_count = self.cached_data.lossy_files.len();
        let total = lossless_count + lossy_count;

        let block = Block::default()
            .title(format!(" Files ({}) ", total))
            .title_style(Style::default().fg(if total > 0 {
                Color::Yellow
            } else {
                Color::DarkGray
            }))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow));

        let inner = render_pane(f, area, block);

        // Populate click targets for file list
        self.click_targets.clear();
        self.click_targets.set_list_area(inner);
        let visible_height = inner.height as usize;
        for (vis_idx, entry_idx) in (self.scroll..).take(visible_height).enumerate() {
            if entry_idx >= total {
                break;
            }
            self.click_targets
                .add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
        }

        if total == 0 {
            let empty = Paragraph::new("No shit format files found")
                .style(Style::default().fg(Color::DarkGray));
            f.render_widget(empty, inner);
            return;
        }

        // Combine lossless and lossy files for display
        let all_files: Vec<_> = self
            .cached_data
            .lossless_files
            .iter()
            .chain(self.cached_data.lossy_files.iter())
            .collect();

        let entries: Vec<PathEntry> = all_files
            .iter()
            .map(|file| {
                let type_tag = format!("[{}] ", file.file_type);
                let tag_color = if file.is_lossless() {
                    Color::Green
                } else {
                    Color::Cyan
                };
                PathEntry {
                    path: &file.corpus_path,
                    prefix: vec![Span::styled(type_tag, Style::default().fg(tag_color))],
                    suffix: Vec::new(),
                }
            })
            .collect();

        render_file_path_list(f, inner, &entries, self.scroll, self.scroll);
    }

    fn render_controls(&mut self, f: &mut Frame, area: Rect) {
        let has_lossless = self.cached_data.has_lossless();
        let has_lossy = self.cached_data.has_lossy();
        let has_both = has_lossless && has_lossy;

        let block = Block::default().borders(Borders::TOP);
        let inner = render_pane(f, area, block);

        if inner.height < 2 {
            return;
        }

        let button_area = Rect { height: 1, ..inner };
        let hint_area = Rect {
            y: inner.y + inner.height - 1,
            height: 1,
            ..inner
        };

        // Build button constraints and names dynamically
        let mut button_names: Vec<&str> = Vec::new();
        if has_lossless {
            button_names.push("remux_lossless");
        }
        if has_lossy {
            button_names.push("transcode_lossy");
        }
        if has_both {
            button_names.push("convert_all");
        }
        button_names.push("cancel");

        let n = button_names.len();
        let constraints: Vec<Constraint> = (0..n).map(|_| Constraint::Ratio(1, n as u32)).collect();

        let button_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(button_area);

        // Track button rects and render each button
        self.button_rects.clear();
        for (i, &name) in button_names.iter().enumerate() {
            self.button_rects.set(name, button_chunks[i]);

            let (label, style) = match name {
                "remux_lossless" => {
                    let label =
                        format!(" Remux {} to FLAC ", self.cached_data.lossless_files.len());
                    let style = if self.selected_button == SelectedButton::RemuxLossless {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Green)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Green)
                    };
                    (label, style)
                }
                "transcode_lossy" => {
                    let label = if self.cached_data.lossy_to_flac {
                        format!(" Capture {} to FLAC ", self.cached_data.lossy_files.len())
                    } else {
                        format!(
                            " Transcode {} to Opus ({} kbps) ",
                            self.cached_data.lossy_files.len(),
                            self.cached_data.opus_bitrate_kbps
                        )
                    };
                    let style = if self.selected_button == SelectedButton::TranscodeLossy {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Cyan)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    (label, style)
                }
                "convert_all" => {
                    let label = " Convert All ".to_string();
                    let style = if self.selected_button == SelectedButton::ConvertAll {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::Yellow)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Yellow)
                    };
                    (label, style)
                }
                "cancel" => {
                    let label = " Cancel ".to_string();
                    let style = if self.selected_button == SelectedButton::Cancel {
                        Style::default()
                            .fg(Color::Black)
                            .bg(Color::White)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    (label, style)
                }
                _ => continue,
            };

            let text = Paragraph::new(label)
                .style(style)
                .alignment(ratatui::layout::Alignment::Center);
            f.render_widget(text, button_chunks[i]);
        }

        // Hints
        let mut hints = Vec::new();
        let show_bitrate_hint = !self.cached_data.lossy_to_flac
            && (self.selected_button == SelectedButton::TranscodeLossy
                || self.selected_button == SelectedButton::ConvertAll)
            && has_lossy;
        if show_bitrate_hint {
            hints.push(Span::styled(
                " [\u{2190}/\u{2192}] adjust bitrate",
                Style::default().fg(Color::DarkGray),
            ));
        }
        hints.push(Span::styled(
            " [Tab] switch buttons",
            Style::default().fg(Color::DarkGray),
        ));

        let hint_para =
            Paragraph::new(Line::from(hints)).alignment(ratatui::layout::Alignment::Center);
        f.render_widget(hint_para, hint_area);
    }
}
