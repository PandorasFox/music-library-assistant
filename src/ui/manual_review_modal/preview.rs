//! Manual Review Modal UI
//!
//! Two-pane horizontal layout for reviewing groups of files that need
//! manual intervention (stashing or tag editing).
//!
//! Navigation:
//! - Tab/Shift+Tab: Navigate between groups
//! - Up/Down: Navigate files within current group
//! - S: Stash selected file (opens confirmation popup)
//! - T: Open tag editor for selected file (individual mode)
//! - Shift+T: Open tag editor for all files in group (aggregated mode)
//! - Ctrl+R: Show transaction review
//! - Esc: Cancel and return to insights

use crate::ui::input::InputAction;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use super::types::{ManualReviewData, ReviewKind};
use crate::ui::helpers::{render_pane, truncate_left, truncate_right};
use crate::ui::widgets::{PathField, CURSOR_STYLE};

/// Actions returned from the manual review modal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManualReviewAction {
    /// No action needed.
    None,
    /// Cancel and return to insights.
    Cancel,
    /// User pressed S — request stash confirmation popup.
    RequestStash,
    /// User confirmed stash in popup.
    ConfirmStash,
    /// User confirmed stash and wants to advance to next group.
    ConfirmStashAndAdvance,
    /// User cancelled stash popup.
    CancelStash,
    /// Open tag editor for the selected file (individual mode).
    OpenTagEditorIndividual,
    /// Open tag editor for all files in group (aggregated mode).
    OpenTagEditorAggregated,
    /// Navigate to next/prev group (true = forward).
    NavigateGroup(bool),
    /// Show transaction review.
    ShowReview,
    /// Mark current group as expected duplicate (suppress future signals).
    MarkExpectedDuplicate,
}

/// Tracks which button is focused in the stash confirmation popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StashConfirmButton {
    #[default]
    Cancel,
    Confirm,
    ConfirmAndAdvance,
}

impl StashConfirmButton {
    fn next(self) -> Self {
        match self {
            Self::Cancel => Self::Confirm,
            Self::Confirm => Self::ConfirmAndAdvance,
            Self::ConfirmAndAdvance => Self::ConfirmAndAdvance,
        }
    }

    fn prev(self) -> Self {
        match self {
            Self::Cancel => Self::Cancel,
            Self::Confirm => Self::Cancel,
            Self::ConfirmAndAdvance => Self::Confirm,
        }
    }
}

/// State for the manual review modal.
pub struct ManualReviewState {
    /// What kind of review.
    pub kind: ReviewKind,
    /// Cached review data.
    pub data: ManualReviewData,
    /// Index of the current group being reviewed.
    pub current_group: usize,
    /// Cursor position within the current group's file list.
    pub file_cursor: usize,
    /// Whether the stash confirmation popup is showing.
    pub stash_confirm: Option<StashConfirmButton>,
}

impl ManualReviewState {
    /// Create a new manual review state.
    pub fn new(kind: ReviewKind, data: ManualReviewData) -> Self {
        Self {
            kind,
            data,
            current_group: 0,
            file_cursor: 0,
            stash_confirm: None,
        }
    }

    /// Header suffix for the title bar.
    pub fn header_suffix(&self) -> &'static str {
        self.kind.title()
    }

    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.current_group_ref()
            .and_then(|g| g.files.get(self.file_cursor))
            .map(|f| f.corpus_path.as_str())
    }

    /// Get a reference to the current group.
    pub(crate) fn current_group_ref(&self) -> Option<&super::types::ReviewGroup> {
        self.data.groups.get(self.current_group)
    }

    /// Get inodes of all non-stashed files in the current group.
    pub fn current_group_inodes(&self) -> Vec<i64> {
        self.current_group_ref()
            .map(|g| {
                g.files
                    .iter()
                    .filter(|f| !f.stashed)
                    .map(|f| f.inode)
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Get the currently selected file entry.
    pub fn selected_file(&self) -> Option<&super::types::ReviewFileEntry> {
        self.current_group_ref()
            .and_then(|g| g.files.get(self.file_cursor))
    }

    /// Mark a file as stashed.
    pub fn mark_stashed(&mut self, group_idx: usize, file_idx: usize) {
        if let Some(group) = self.data.groups.get_mut(group_idx) {
            if let Some(file) = group.files.get_mut(file_idx) {
                file.stashed = true;
            }
        }
    }

    /// Whether the current group is the last one.
    fn is_last_group(&self) -> bool {
        self.current_group + 1 >= self.data.groups.len()
    }

    /// Handle input action.
    pub fn handle_input(&mut self, action: &InputAction) -> ManualReviewAction {
        // If stash confirmation popup is showing, handle popup actions
        if let Some(ref mut button) = self.stash_confirm {
            return match action {
                InputAction::Cancel => {
                    self.stash_confirm = None;
                    ManualReviewAction::CancelStash
                }
                InputAction::NavLeft => {
                    *button = button.prev();
                    ManualReviewAction::None
                }
                InputAction::NavRight => {
                    *button = button.next();
                    ManualReviewAction::None
                }
                InputAction::Confirm | InputAction::Toggle => {
                    let action = match *button {
                        StashConfirmButton::Cancel => ManualReviewAction::CancelStash,
                        StashConfirmButton::Confirm => ManualReviewAction::ConfirmStash,
                        StashConfirmButton::ConfirmAndAdvance => {
                            ManualReviewAction::ConfirmStashAndAdvance
                        }
                    };
                    self.stash_confirm = None;
                    action
                }
                _ => ManualReviewAction::None,
            };
        }

        // Normal action handling
        match action {
            InputAction::Cancel => ManualReviewAction::Cancel,

            // File navigation
            InputAction::NavUp => {
                self.file_cursor = self.file_cursor.saturating_sub(1);
                ManualReviewAction::None
            }
            InputAction::NavDown => {
                if let Some(group) = self.current_group_ref() {
                    let max = group.files.len().saturating_sub(1);
                    if self.file_cursor < max {
                        self.file_cursor += 1;
                    }
                }
                ManualReviewAction::None
            }

            // Group navigation
            InputAction::CycleNext => {
                if self.is_last_group() {
                    // Tab past last group → show review
                    ManualReviewAction::ShowReview
                } else {
                    ManualReviewAction::NavigateGroup(true)
                }
            }
            InputAction::CyclePrev => ManualReviewAction::NavigateGroup(false),

            // Stash
            InputAction::Char('s') | InputAction::Char('S') => {
                // Check if selected file is already stashed
                if let Some(file) = self.selected_file() {
                    if file.stashed {
                        return ManualReviewAction::None;
                    }
                }
                self.stash_confirm = Some(StashConfirmButton::Confirm);
                ManualReviewAction::RequestStash
            }

            // Tag editor
            InputAction::Char('t') => {
                if self.kind.supports_tag_edit() {
                    ManualReviewAction::OpenTagEditorIndividual
                } else {
                    ManualReviewAction::None
                }
            }
            InputAction::Char('T') => {
                if self.kind.supports_tag_edit() {
                    ManualReviewAction::OpenTagEditorAggregated
                } else {
                    ManualReviewAction::None
                }
            }

            // Mark expected duplicate
            InputAction::FlagValue => ManualReviewAction::MarkExpectedDuplicate,

            // Transaction review
            InputAction::Shortcut('r') => ManualReviewAction::ShowReview,

            _ => ManualReviewAction::None,
        }
    }
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the manual review modal.
pub fn render(f: &mut Frame, area: Rect, state: &ManualReviewState) {
    // Clear background
    f.render_widget(Clear, area);

    // Layout: content + controls hint
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),   // Content
            Constraint::Length(1), // Controls hint
        ])
        .split(area);

    render_content(f, main_chunks[0], state);
    render_controls_hint(f, main_chunks[1], state);

    // Render stash confirmation popup overlay if active
    if let Some(button) = state.stash_confirm {
        render_stash_confirm_popup(f, area, state, button);
    }
}

fn render_content(f: &mut Frame, area: Rect, state: &ManualReviewState) {
    // Two-pane horizontal: file list (left) | detail pane (right)
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    render_file_list(f, panes[0], state);
    render_detail_pane(f, panes[1], state);
}

fn render_file_list(f: &mut Frame, area: Rect, state: &ManualReviewState) {
    let group = state.current_group_ref();
    let group_count = state.data.groups.len();
    let group_idx = state.current_group + 1;

    let title = if let Some(g) = group {
        format!(
            " Group {}/{}: {} ({} files) ",
            group_idx,
            group_count,
            truncate_right(&g.label, 30),
            g.files.len()
        )
    } else {
        " No groups ".to_string()
    };

    let block = Block::default()
        .title(title)
        .title_style(Style::default().fg(Color::Cyan))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = render_pane(f, area, block);

    let Some(group) = group else {
        let empty =
            Paragraph::new("No groups to review").style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, inner);
        return;
    };

    if group.files.is_empty() {
        let empty = Paragraph::new("No files in group").style(Style::default().fg(Color::DarkGray));
        f.render_widget(empty, inner);
        return;
    }

    let visible_lines = inner.height as usize;
    let max_width = inner.width as usize;

    // Calculate scroll window centered on cursor
    let start = if state.file_cursor >= visible_lines {
        state.file_cursor - visible_lines + 1
    } else {
        0
    };

    let items: Vec<ListItem> = group
        .files
        .iter()
        .skip(start)
        .take(visible_lines)
        .enumerate()
        .map(|(vis_idx, file)| {
            let actual_idx = start + vis_idx;
            let is_selected = actual_idx == state.file_cursor;

            let style = if file.stashed {
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::CROSSED_OUT)
            } else if is_selected {
                CURSOR_STYLE
            } else {
                Style::default().fg(Color::White)
            };

            let prefix = if file.stashed {
                "✗ "
            } else if is_selected {
                "▸ "
            } else {
                "  "
            };
            let path = truncate_left(&file.corpus_path, max_width.saturating_sub(4));

            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(path, style),
            ]))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

fn render_detail_pane(f: &mut Frame, area: Rect, state: &ManualReviewState) {
    let block = Block::default()
        .title(" Details ")
        .title_style(Style::default().fg(Color::DarkGray))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = render_pane(f, area, block);

    let group = state.current_group_ref();
    let file = state.selected_file();
    let max_lines = inner.height as usize;

    let mut lines = Vec::new();

    // Group info
    if let Some(g) = group {
        lines.push(Line::from(vec![
            Span::styled(
                "Group: ",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(&g.label, Style::default().fg(Color::White)),
        ]));
        lines.push(Line::from(""));
    }

    let label_style = Style::default().fg(Color::DarkGray);
    let value_style = Style::default().fg(Color::White);

    // Selected file info
    if let Some(file) = file {
        lines.extend(
            PathField::new(Span::styled("Path: ", label_style), &file.corpus_path)
                .style(value_style)
                .render_lines(inner.width),
        );

        // Context line (kind-specific: deploy path, quality score, tag signature)
        lines.push(Line::from(vec![
            Span::styled("Context: ", label_style),
            Span::styled(&file.context, Style::default().fg(Color::Yellow)),
        ]));

        // Audio metadata
        if let Some(ref meta) = file.meta {
            lines.push(Line::from(""));

            // Format + duration + bitrate + sample rate on compact lines
            lines.push(Line::from(vec![
                Span::styled("Format: ", label_style),
                Span::styled(meta.file_type.to_uppercase(), value_style),
            ]));

            if let Some(dur) = meta.duration_ms {
                let secs = dur / 1000;
                let mins = secs / 60;
                let rem = secs % 60;
                lines.push(Line::from(vec![
                    Span::styled("Duration: ", label_style),
                    Span::styled(format!("{}:{:02}", mins, rem), value_style),
                ]));
            }

            if let Some(br) = meta.bitrate_kbps {
                lines.push(Line::from(vec![
                    Span::styled("Bitrate: ", label_style),
                    Span::styled(format!("{} kbps", br), value_style),
                ]));
            }

            if let Some(sr) = meta.sample_rate {
                let display = if sr >= 1000 && sr % 1000 == 0 {
                    format!("{} kHz", sr / 1000)
                } else if sr >= 1000 {
                    format!("{:.1} kHz", sr as f64 / 1000.0)
                } else {
                    format!("{} Hz", sr)
                };
                lines.push(Line::from(vec![
                    Span::styled("Sample rate: ", label_style),
                    Span::styled(display, value_style),
                ]));
            }

            if meta.file_size > 0 {
                let size_str = if meta.file_size >= 1_048_576 {
                    format!("{:.1} MB", meta.file_size as f64 / 1_048_576.0)
                } else {
                    format!("{:.0} KB", meta.file_size as f64 / 1024.0)
                };
                lines.push(Line::from(vec![
                    Span::styled("Size: ", label_style),
                    Span::styled(size_str, value_style),
                ]));
            }

            let art_label = if meta.has_pictures { "Yes" } else { "No" };
            let art_style = if meta.has_pictures {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            lines.push(Line::from(vec![
                Span::styled("Album art: ", label_style),
                Span::styled(art_label, art_style),
            ]));

            // Tags section
            if !meta.tags.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Tags:",
                    Style::default()
                        .fg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                )));

                let tag_budget = max_lines.saturating_sub(lines.len());
                for (name, value) in meta.tags.iter().take(tag_budget) {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {}: ", name), label_style),
                        Span::styled(value, value_style),
                    ]));
                }
                let remaining = meta.tags.len().saturating_sub(tag_budget);
                if remaining > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("  ... +{} more", remaining),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        if file.stashed {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "STASHED — staged for removal",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )));
        }
    } else {
        lines.push(Line::from(Span::styled(
            "No file selected",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Group summary — count of stashed files
    if let Some(g) = group {
        let stashed_count = g.files.iter().filter(|f| f.stashed).count();
        if stashed_count > 0 {
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                format!(
                    "{}/{} files stashed in this group",
                    stashed_count,
                    g.files.len()
                ),
                Style::default().fg(Color::Yellow),
            )));
        }
    }

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

fn render_controls_hint(f: &mut Frame, area: Rect, state: &ManualReviewState) {
    let mut hints = vec![
        Span::styled(" ↑↓", Style::default().fg(Color::Cyan)),
        Span::styled(" files ", Style::default().fg(Color::DarkGray)),
        Span::styled(" Tab/S-Tab", Style::default().fg(Color::Cyan)),
        Span::styled(" groups ", Style::default().fg(Color::DarkGray)),
        Span::styled(" S", Style::default().fg(Color::Cyan)),
        Span::styled(" stash ", Style::default().fg(Color::DarkGray)),
    ];

    if state.kind.supports_tag_edit() {
        hints.push(Span::styled(" T/S-T", Style::default().fg(Color::Cyan)));
        hints.push(Span::styled(" tags ", Style::default().fg(Color::DarkGray)));
    }

    if state.kind == ReviewKind::RedundantDuplicate {
        hints.push(Span::styled(" ^F", Style::default().fg(Color::Cyan)));
        hints.push(Span::styled(
            " expected ",
            Style::default().fg(Color::DarkGray),
        ));
    }

    hints.push(Span::styled(" ^R", Style::default().fg(Color::Cyan)));
    hints.push(Span::styled(
        " review ",
        Style::default().fg(Color::DarkGray),
    ));
    hints.push(Span::styled(" Esc", Style::default().fg(Color::Cyan)));
    hints.push(Span::styled(
        " cancel",
        Style::default().fg(Color::DarkGray),
    ));

    let controls = Paragraph::new(Line::from(hints));
    f.render_widget(controls, area);
}

fn render_stash_confirm_popup(
    f: &mut Frame,
    area: Rect,
    state: &ManualReviewState,
    button: StashConfirmButton,
) {
    let file_label = state
        .selected_file()
        .map(|f| truncate_left(&f.corpus_path, 40))
        .unwrap_or_else(|| "<unknown>".to_string());

    // Build button spans
    let cancel_style = if button == StashConfirmButton::Cancel {
        Style::default()
            .fg(Color::Black)
            .bg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    let confirm_style = if button == StashConfirmButton::Confirm {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Cyan)
    };
    let advance_style = if button == StashConfirmButton::ConfirmAndAdvance {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Green)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };

    let content = vec![
        Line::from(""),
        Line::from(Span::styled(
            "Add stash to transaction?",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("File: ", Style::default().fg(Color::DarkGray)),
            Span::styled(&file_label, Style::default().fg(Color::White)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Stash: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("stash/{}/", state.kind.stash_name()),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(""),
        Line::from(""),
        Line::from(vec![
            Span::styled(" Cancel ", cancel_style),
            Span::raw("  "),
            Span::styled(" Confirm ", confirm_style),
            Span::raw("  "),
            Span::styled(" Confirm & Next Group ", advance_style),
        ]),
    ];

    use crate::ui::widgets::{Modal, ModalStyle};

    Modal::new()
        .title(" Stash File ")
        .content(content)
        .fixed_size(55, 12)
        .style(ModalStyle::info())
        .centered()
        .render(f, area);
}
