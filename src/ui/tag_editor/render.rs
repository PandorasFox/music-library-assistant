//! Tag Editor Rendering
//!
//! All UI rendering functions for the tag editor.

#![allow(dead_code)]

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::state::TagEditorState;
use super::types::{FieldEditState, GroupedChange, TagChange};

impl TagEditorState {
    /// Render the tag editor
    pub fn render(&mut self, f: &mut Frame, area: Rect, status_message: Option<&str>) {
        // Layout: info pane | 3-column | status box
        let editor_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),  // Info pane
                Constraint::Min(15),    // 3-column area
                Constraint::Length(12), // Status box
            ])
            .split(area);

        self.render_info_pane(f, editor_layout[0]);
        self.render_three_column(f, editor_layout[1]);
        self.render_status_box(f, editor_layout[2], status_message);
    }

    fn render_info_pane(&self, f: &mut Frame, area: Rect) {
        let current_track = &self.tracks[self.current_track_idx];

        // Format duration
        let duration_str = if let Some(duration_ms) = current_track.duration_ms {
            let total_seconds = duration_ms / 1000;
            let minutes = total_seconds / 60;
            let seconds = total_seconds % 60;
            format!("{}:{:02}", minutes, seconds)
        } else {
            "Unknown".to_string()
        };

        // Format file size
        let size_str = if current_track.file_size < 1024 {
            format!("{} B", current_track.file_size)
        } else if current_track.file_size < 1024 * 1024 {
            format!("{:.1} KB", current_track.file_size as f64 / 1024.0)
        } else {
            format!(
                "{:.2} MB",
                current_track.file_size as f64 / (1024.0 * 1024.0)
            )
        };

        let sample_rate_str = current_track
            .sample_rate
            .map(|sr| format!("{} Hz", sr))
            .unwrap_or_else(|| "Unknown".to_string());

        let bitrate_str = current_track
            .bitrate_kbps
            .map(|b| format!("{} kbps", b))
            .unwrap_or_else(|| "Unknown".to_string());

        let info_lines = vec![
            Line::from(format!("Path: {}", current_track.path)),
            Line::from(format!(
                "Format: {} | Size: {} | Duration: {} | Bitrate: {} | Sample Rate: {}",
                current_track.file_type.to_uppercase(),
                size_str,
                duration_str,
                bitrate_str,
                sample_rate_str
            )),
            Line::from(format!(
                "Source: {} | Inode: {}",
                current_track.source, current_track.inode
            )),
        ];

        let info_para = Paragraph::new(info_lines).block(
            Block::default().borders(Borders::ALL).title(format!(
                "File Info [Track {}/{}]",
                self.current_track_idx + 1,
                self.tracks.len()
            )),
        );
        f.render_widget(info_para, area);
    }

    fn render_three_column(&self, f: &mut Frame, area: Rect) {
        let three_column = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(30), // Track list
                Constraint::Percentage(60), // Tag editor
                Constraint::Percentage(10), // Search panel
            ])
            .split(area);

        self.render_track_list(f, three_column[0]);
        self.render_tag_fields(f, three_column[1]);
        self.render_search_panel(f, three_column[2]);
    }

    fn render_track_list(&self, f: &mut Frame, area: Rect) {
        let track_items: Vec<ListItem> = self
            .tracks
            .iter()
            .enumerate()
            .map(|(idx, track)| {
                let prefix = if idx == self.current_track_idx {
                    ">> "
                } else {
                    "   "
                };
                let artist = track.artist.as_deref().unwrap_or("Unknown");
                let title = track.title.as_deref().unwrap_or("Unknown");
                ListItem::new(Line::from(format!("{}{} - {}", prefix, artist, title))).style(
                    if idx == self.current_track_idx {
                        Style::default().bg(Color::DarkGray)
                    } else {
                        Style::default()
                    },
                )
            })
            .collect();

        let track_list =
            List::new(track_items).block(Block::default().borders(Borders::ALL).title("Tracks"));
        f.render_widget(track_list, area);
    }

    fn render_tag_fields(&self, f: &mut Frame, area: Rect) {
        let fields = &self.tag_fields[self.current_track_idx];
        let field_lines: Vec<Line> = fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                let name_display =
                    if is_current && matches!(self.field_edit_state, FieldEditState::EditingName) {
                        self.name_buffer.clone() + "_"
                    } else {
                        field.name.clone()
                    };

                let value_display =
                    if is_current && matches!(self.field_edit_state, FieldEditState::EditingValue) {
                        self.value_buffer.clone() + "_"
                    } else {
                        field.value.clone()
                    };

                let fill_button =
                    if is_current && field.editable && !field.is_unique_per_track && !field.value.is_empty()
                    {
                        " [F]"
                    } else {
                        ""
                    };

                let line_text = format!("{:18} : {}{}", name_display, value_display, fill_button);

                let style = if is_current {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };

                Line::from(line_text).style(style)
            })
            .collect();

        let tag_para = Paragraph::new(field_lines)
            .block(Block::default().borders(Borders::ALL).title("Tag Editor"));
        f.render_widget(tag_para, area);
    }

    fn render_search_panel(&self, f: &mut Frame, area: Rect) {
        let search_para = Paragraph::new(vec![Line::from("(TODO)")])
            .block(Block::default().borders(Borders::ALL).title("Search"))
            .alignment(Alignment::Center);
        f.render_widget(search_para, area);
    }

    fn render_status_box(&self, f: &mut Frame, area: Rect, status_message: Option<&str>) {
        let mut status_lines = vec![];

        if let Some(msg) = status_message {
            status_lines.push(Line::from(msg).style(Style::default().fg(Color::Yellow)));
            status_lines.push(Line::from(""));
        }

        status_lines.extend(vec![
            Line::from("Navigation: Tab/Shift+Tab = next/prev track | Left/Right = toggle name/value | Up/Down = navigate fields | Enter = edit/commit"),
            Line::from(""),
            Line::from("Actions: F = fill to all | Ctrl+U = clear | Esc = exit | Tab past last track to save all"),
        ]);

        let status_para = Paragraph::new(status_lines)
            .block(Block::default().borders(Borders::ALL).title("Status"));
        f.render_widget(status_para, area);
    }
}

// ============================================================================
// Modal Rendering
// ============================================================================

/// Render the save confirmation modal
pub fn render_save_confirmation_modal(f: &mut Frame, area: Rect, selected_button: usize) {
    use super::super::helpers::centered_rect;

    let popup_area = centered_rect(60, 30, area);

    // Clear background
    let clear_block = Block::default().style(Style::default().bg(Color::Reset));
    f.render_widget(clear_block, area);

    // Modal box
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title("Save Changes?")
        .border_style(Style::default().fg(Color::Yellow));

    let inner = modal_block.inner(popup_area);
    f.render_widget(modal_block, popup_area);

    let buttons = [
        "[ Save All Changes ]",
        "[ Save All & Next Set ]",
        "[ Return to Editing ]",
    ];

    let mut button_lines: Vec<Line> = vec![
        Line::from(""),
        Line::from("All edits will be written to files."),
        Line::from(""),
    ];

    for (i, label) in buttons.iter().enumerate() {
        let style = if i == selected_button {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        button_lines.push(Line::from(*label).style(style));
        if i < buttons.len() - 1 {
            button_lines.push(Line::from(""));
        }
    }

    button_lines.push(Line::from(""));
    button_lines.push(
        Line::from("Use Left/Right to select, Enter to confirm, Esc to cancel")
            .style(Style::default().fg(Color::DarkGray)),
    );

    let paragraph = Paragraph::new(button_lines).alignment(Alignment::Center);
    f.render_widget(paragraph, inner);
}

/// Render the change preview modal
pub fn render_change_preview_modal(
    f: &mut Frame,
    area: Rect,
    grouped_changes: &[GroupedChange],
    single_changes: &[TagChange],
    scroll_offset: usize,
) {
    use super::super::helpers::centered_rect;

    let modal_area = centered_rect(80, 80, area);

    // Clear background
    let clear_block = Block::default().style(Style::default().bg(Color::Reset));
    f.render_widget(clear_block, area);

    // Modal box
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title("Review Changes Before Saving")
        .border_style(Style::default().fg(Color::Yellow));

    let inner = modal_block.inner(modal_area);
    f.render_widget(modal_block, modal_area);

    // Build lines for preview
    let mut lines = Vec::new();

    let total_changes = grouped_changes
        .iter()
        .map(|g| g.track_indices.len())
        .sum::<usize>()
        + single_changes.len();
    lines.push(
        Line::from(format!("Total changes: {}", total_changes)).style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        ),
    );
    lines.push(Line::from(""));

    // Grouped changes section
    if !grouped_changes.is_empty() {
        lines.push(
            Line::from("Common Changes (multiple tracks):").style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Green),
            ),
        );
        lines.push(Line::from(""));

        for group in grouped_changes {
            let track_list = if group.track_indices.len() <= 5 {
                group
                    .track_indices
                    .iter()
                    .map(|i| (i + 1).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            } else {
                format!("{} tracks", group.track_indices.len())
            };

            let line_text = format!(
                "  [{}] {}: '{}' -> '{}'",
                track_list,
                group.field_name,
                if group.old_value.is_empty() {
                    "(empty)"
                } else {
                    &group.old_value
                },
                if group.new_value.is_empty() {
                    "(empty)"
                } else {
                    &group.new_value
                }
            );
            lines.push(Line::from(line_text).style(Style::default().fg(Color::Cyan)));
        }
        lines.push(Line::from(""));
    }

    // Single-track changes section
    if !single_changes.is_empty() {
        lines.push(
            Line::from("Individual Track Changes:").style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .fg(Color::Yellow),
            ),
        );
        lines.push(Line::from(""));

        for change in single_changes {
            let line_text = format!(
                "  Track {}: {}: '{}' -> '{}'",
                change.track_idx + 1,
                change.field_name,
                if change.old_value.is_empty() {
                    "(empty)"
                } else {
                    &change.old_value
                },
                if change.new_value.is_empty() {
                    "(empty)"
                } else {
                    &change.new_value
                }
            );
            lines.push(Line::from(line_text).style(Style::default().fg(Color::White)));
        }
        lines.push(Line::from(""));
    }

    // Instructions
    lines.push(Line::from(""));
    lines.push(
        Line::from("Press Enter/Y to confirm and save, Esc/N to cancel")
            .style(Style::default().fg(Color::DarkGray)),
    );
    lines.push(
        Line::from("Use Up/Down or PgUp/PgDn to scroll")
            .style(Style::default().fg(Color::DarkGray)),
    );

    // Apply scroll offset
    let max_scroll = lines.len().saturating_sub(inner.height as usize);
    let clamped_offset = scroll_offset.min(max_scroll);
    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(clamped_offset)
        .take(inner.height as usize)
        .collect();

    let paragraph = Paragraph::new(visible_lines);
    f.render_widget(paragraph, inner);
}
