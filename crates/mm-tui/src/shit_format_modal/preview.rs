//! Shit Format Resolution Preview UI
//!
//! Shows non-Vorbis container format files split into:
//! - Lossless (WAV, AIFF, APE, WV) → Remux to FLAC
//! - Lossy (MP3, M4A, AAC, WMA) → Transcode to Opus
//!
//! - Shift+Up/Down: Switch focus between list and buttons
//! - Up/Down: Navigate file list (when list focused)
//! - Left/Right: Adjust Opus bitrate (when on lossy button) / button navigation
//! - Tab: Cycle between buttons
//! - Enter: Execute selected button action
//! - Escape: Cancel

use crate::action_handlers::witness::ConfirmationGesture;
use crate::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
    Frame,
};

use super::types::{ShitFormatButton, ShitFormatButtonCtx, ShitFormatModalData};
use crate::helpers::{render_pane, truncate_right};
use crate::widgets::FocusPane;
use crate::widgets::modal_frame::{ContentLayout, FrameInputResult, FrameState, ModalFrame, ModalFrameCore};
use crate::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

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

// ============================================================================
// State
// ============================================================================

/// State for the shit format resolution modal.
#[derive(Debug)]
pub struct ShitFormatPreviewState {
    /// Cached modal data (loaded once on init).
    pub cached_data: ShitFormatModalData,
    /// Cursor position for the file list.
    pub cursor: usize,
    /// Shared frame state (focus, buttons, click targets).
    pub frame: FrameState<ShitFormatButton>,
}

impl ShitFormatPreviewState {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        let lossless_len = self.cached_data.lossless_files.len();
        if self.cursor < lossless_len {
            self.cached_data
                .lossless_files
                .get(self.cursor)
                .map(|f| f.corpus_path.as_str())
        } else {
            self.cached_data
                .lossy_files
                .get(self.cursor - lossless_len)
                .map(|f| f.corpus_path.as_str())
        }
    }

    /// Create a new preview state with cached data.
    pub fn new(cached_data: ShitFormatModalData) -> Self {
        let mut frame = FrameState::new();
        // Default to first available action button
        if cached_data.has_lossless() {
            frame.buttons.selected = ShitFormatButton::RemuxLossless;
        } else if cached_data.has_lossy() {
            frame.buttons.selected = ShitFormatButton::TranscodeLossy;
        }

        Self {
            cached_data,
            cursor: 0,
            frame,
        }
    }

    fn button_ctx(&self) -> ShitFormatButtonCtx {
        ShitFormatButtonCtx {
            lossless_count: self.cached_data.lossless_files.len(),
            lossy_count: self.cached_data.lossy_files.len(),
            has_lossless: self.cached_data.has_lossless(),
            has_lossy: self.cached_data.has_lossy(),
            lossy_to_flac: self.cached_data.lossy_to_flac,
            opus_bitrate_kbps: self.cached_data.opus_bitrate_kbps,
        }
    }

    /// Handle a mouse click at (x, y).
    pub(crate) fn handle_click(
        &mut self,
        x: u16,
        y: u16,
        _gesture: &ConfirmationGesture,
    ) -> Option<ShitFormatPreviewAction> {
        let ctx = self.button_ctx();
        if let Some(action) = self.frame.buttons.handle_click(x, y, &ctx) {
            self.frame.focus_pane = FocusPane::Buttons;
            return Some(action);
        }
        if let Some(id) = self.frame.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if idx < self.cached_data.total_count() {
                    self.frame.focus_pane = FocusPane::List;
                    self.cursor = idx;
                }
            }
        }
        None
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> ShitFormatPreviewAction {
        // Tab cycles between buttons (works regardless of focus pane)
        match action {
            InputAction::CycleNext => {
                let ctx = self.button_ctx();
                self.frame.buttons.nav_right(&ctx);
                return ShitFormatPreviewAction::None;
            }
            InputAction::CyclePrev => {
                let ctx = self.button_ctx();
                self.frame.buttons.nav_left(&ctx);
                return ShitFormatPreviewAction::None;
            }
            _ => {}
        }

        // NavLeft/NavRight: bitrate adjustment when on lossy button in Buttons pane
        if self.frame.focus_pane == FocusPane::Buttons {
            let on_lossy = matches!(
                self.frame.buttons.selected,
                ShitFormatButton::TranscodeLossy | ShitFormatButton::ConvertAll
            );
            if on_lossy && !self.cached_data.lossy_to_flac {
                match action {
                    InputAction::NavLeft => {
                        self.cached_data.decrease_bitrate();
                        return ShitFormatPreviewAction::None;
                    }
                    InputAction::NavRight => {
                        self.cached_data.increase_bitrate();
                        return ShitFormatPreviewAction::None;
                    }
                    _ => {}
                }
            }
        }

        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => a,
            FrameInputResult::Consumed | FrameInputResult::Unhandled => {
                ShitFormatPreviewAction::None
            }
        }
    }

    /// Render the shit format resolution modal.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        self.render_frame(f, area);
    }

    // === Private rendering helpers ===

    fn render_lossless_section(&self, f: &mut Frame, area: Rect) {
        let has_files = self.cached_data.has_lossless();
        let color = if has_files { Color::Green } else { Color::DarkGray };

        let block = Block::default()
            .title(" Lossless \u{2192} FLAC ")
            .title_style(Style::default().fg(color))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color));

        let inner = render_pane(f, area, block);

        if !has_files {
            f.render_widget(
                Paragraph::new("No lossless files").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(2), Constraint::Min(2)])
            .split(inner);

        f.render_widget(
            Paragraph::new("Remux to FLAC (lossless)").style(Style::default().fg(Color::DarkGray)),
            chunks[0],
        );

        let breakdown = self.cached_data.lossless_breakdown();
        let items: Vec<ListItem> = breakdown
            .iter()
            .map(|(ftype, count)| {
                ListItem::new(format!("  {}: {}", ftype, count))
                    .style(Style::default().fg(Color::White))
            })
            .collect();
        f.render_widget(List::new(items), chunks[1]);
    }

    fn render_lossy_section(&self, f: &mut Frame, area: Rect) {
        let has_files = self.cached_data.has_lossy();
        let lossy_to_flac = self.cached_data.lossy_to_flac;
        let color = if has_files { Color::Cyan } else { Color::DarkGray };

        let title = if lossy_to_flac {
            " Lossy \u{2192} FLAC (lossy capture) "
        } else {
            " Lossy \u{2192} Opus "
        };

        let block = Block::default()
            .title(title)
            .title_style(Style::default().fg(color))
            .borders(Borders::ALL)
            .border_style(Style::default().fg(color));

        let inner = render_pane(f, area, block);

        if !has_files {
            f.render_widget(
                Paragraph::new("No lossy files").style(Style::default().fg(Color::DarkGray)),
                inner,
            );
            return;
        }

        if lossy_to_flac {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(2)])
                .split(inner);

            f.render_widget(
                Paragraph::new("Capture decoded waveform to FLAC")
                    .style(Style::default().fg(Color::DarkGray)),
                chunks[0],
            );

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
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(3), Constraint::Min(2)])
                .split(inner);

            let bitrate = self.cached_data.opus_bitrate_kbps;
            let ratio = (bitrate as f64 - 32.0) / (512.0 - 32.0);
            let gauge = Gauge::default()
                .block(
                    Block::default()
                        .title("Opus Bitrate")
                        .borders(Borders::NONE),
                )
                .gauge_style(Style::default().fg(Color::Cyan).bg(Color::DarkGray))
                .ratio(ratio)
                .label(format!("{} kbps", bitrate));
            f.render_widget(gauge, chunks[0]);

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
}

// ============================================================================
// ModalFrame Implementation
// ============================================================================

impl ModalFrameCore for ShitFormatPreviewState {
    type Button = ShitFormatButton;

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit {
            list_percent: 60,
            info_height: 3,
        }
    }

    fn list_title(&self) -> String {
        format!(" Files ({}) ", self.cached_data.total_count())
    }

    fn empty_message(&self) -> &'static str {
        "No shit format files found"
    }

    fn frame_state(&self) -> &FrameState<ShitFormatButton> {
        &self.frame
    }
    fn frame_state_mut(&mut self) -> &mut FrameState<ShitFormatButton> {
        &mut self.frame
    }
    fn cursor(&self) -> usize {
        self.cursor
    }
    fn cursor_mut(&mut self) -> &mut usize {
        &mut self.cursor
    }
    fn list_len(&self) -> usize {
        self.cached_data.total_count()
    }
    fn button_ctx(&self) -> ShitFormatButtonCtx {
        ShitFormatPreviewState::button_ctx(self)
    }
    fn escape_action(&self) -> ShitFormatPreviewAction {
        ShitFormatPreviewAction::Cancel
    }
}

impl ModalFrame for ShitFormatPreviewState {
    fn accent_color(&self) -> Color {
        Color::Yellow
    }

    fn controls_hints(&self) -> Vec<Span<'static>> {
        let s = Style::default().fg(Color::DarkGray);
        let mut hints = vec![
            Span::styled("Shift+\u{2191}\u{2193}", s),
            Span::styled(" focus  ", s),
        ];

        let on_lossy = matches!(
            self.frame.buttons.selected,
            ShitFormatButton::TranscodeLossy | ShitFormatButton::ConvertAll
        );
        if on_lossy && !self.cached_data.lossy_to_flac && self.cached_data.has_lossy() {
            hints.push(Span::styled("\u{2190}\u{2192}", s));
            hints.push(Span::styled(" bitrate  ", s));
        } else {
            hints.push(Span::styled("\u{2190}\u{2192}", s));
            hints.push(Span::styled(" select  ", s));
        }

        hints.push(Span::styled("Tab", s));
        hints.push(Span::styled(" switch  ", s));
        hints.push(Span::styled("Enter", s));
        hints.push(Span::styled(" confirm", s));
        hints
    }

    fn render_info_bar(&self, f: &mut Frame, area: Rect) {
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

    fn render_list_item(
        &self,
        idx: usize,
        width: u16,
        is_cursor: bool,
        _is_focused: bool,
    ) -> ListItem<'static> {
        let lossless_len = self.cached_data.lossless_files.len();
        let file = if idx < lossless_len {
            &self.cached_data.lossless_files[idx]
        } else {
            &self.cached_data.lossy_files[idx - lossless_len]
        };

        let type_tag = format!("[{}] ", file.file_type);
        let tag_color = if file.is_lossless() {
            Color::Green
        } else {
            Color::Cyan
        };
        let prefix_len = type_tag.chars().count();
        let path_budget = (width as usize).saturating_sub(prefix_len);
        let path = truncate_right(&file.corpus_path, path_budget);
        let style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };

        ListItem::new(Line::from(vec![
            Span::styled(
                type_tag,
                if is_cursor {
                    CURSOR_STYLE
                } else {
                    Style::default().fg(tag_color)
                },
            ),
            Span::styled(path, style),
        ]))
    }

    fn render_detail(&mut self, f: &mut Frame, area: Rect) {
        // Config pane: lossless section + lossy section
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
            .split(area);

        self.render_lossless_section(f, chunks[0]);
        self.render_lossy_section(f, chunks[1]);
    }
}
