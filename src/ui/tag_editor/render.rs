//! Tag Editor Rendering
//!
//! All UI rendering functions for the tag editor.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
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

    fn render_three_column(&mut self, f: &mut Frame, area: Rect) {
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
        self.render_action_panel(f, three_column[2]);
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

        // Show context-appropriate title
        // TODO: Show detailed conflict info:
        // - Directory path of each conflicting file
        // - Whether non-deploy tags differ between tracks (genre, etc.)
        // - Whether fingerprints match (exact duplicate vs different recordings)
        // - Target deployment path and library name (from health issue metadata)
        let title = if !self.duplicate_groups.is_empty() {
            let group_info = self.current_group_idx
                .map(|idx| format!(" [{}/{}]", idx + 1, self.duplicate_groups.len()))
                .unwrap_or_default();
            format!("Deploy Conflict{}", group_info)
        } else {
            "Tracks".to_string()
        };

        let track_list =
            List::new(track_items).block(Block::default().borders(Borders::ALL).title(title));
        f.render_widget(track_list, area);
    }

    fn render_tag_fields(&mut self, f: &mut Frame, area: Rect) {
        let fields = &self.tag_fields[self.current_track_idx];
        let original_fields = &self.original_tag_fields[self.current_track_idx];

        // Calculate visible height (area height minus 2 for borders)
        let visible_height = area.height.saturating_sub(2) as usize;
        self.tag_visible_height = visible_height;

        // Build all field lines
        let field_lines: Vec<Line> = fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                // Check if this field has been modified from original
                let is_modified = original_fields
                    .iter()
                    .find(|orig| orig.name == field.name)
                    .map(|orig| orig.value != field.value)
                    .unwrap_or(true); // New fields are considered modified

                let name_display =
                    if is_current && matches!(self.field_edit_state, FieldEditState::EditingName) {
                        self.name_buffer.clone() + "_"
                    } else {
                        field.name.clone()
                    };

                // Prepend edit indicator if modified
                let name_with_indicator = if is_modified {
                    format!("✎ {}", name_display)
                } else {
                    format!("  {}", name_display)
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

                let line_text = format!("{:18} : {}{}", name_with_indicator, value_display, fill_button);

                let style = if is_current {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else if is_modified {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };

                Line::from(line_text).style(style)
            })
            .collect();

        // Apply scroll offset - show only visible portion
        let total_fields = field_lines.len();
        let scroll_indicator = if total_fields > visible_height {
            let pos = self.tag_scroll_offset + 1;
            let max = total_fields.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        let visible_lines: Vec<Line> = field_lines
            .into_iter()
            .skip(self.tag_scroll_offset)
            .take(visible_height)
            .collect();

        let tag_para = Paragraph::new(visible_lines)
            .block(Block::default().borders(Borders::ALL).title(format!("Tag Editor{}", scroll_indicator)));
        f.render_widget(tag_para, area);
    }

    fn render_action_panel(&self, f: &mut Frame, area: Rect) {
        use super::types::TagEditorFocus;

        let is_focused = matches!(self.focus, TagEditorFocus::ActionPane);

        // Count pending changes
        let changes = super::state::compute_changes(&self.original_tag_fields, &self.tag_fields);
        let change_count = changes.len();

        let button_style = if is_focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Green)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };

        let lines = vec![
            Line::from(""),
            Line::from(if is_focused { "[ Proceed ]" } else { "  Proceed  " }).style(button_style),
            Line::from(""),
            Line::from(format!("{} changes", change_count)).style(Style::default().fg(Color::DarkGray)),
        ];

        let border_style = if is_focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };

        let action_para = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Action")
                    .border_style(border_style),
            )
            .alignment(Alignment::Center);
        f.render_widget(action_para, area);
    }

    fn render_status_box(&self, f: &mut Frame, area: Rect, status_message: Option<&str>) {
        let mut status_lines = vec![];

        if let Some(msg) = status_message {
            status_lines.push(Line::from(msg).style(Style::default().fg(Color::Yellow)));
            status_lines.push(Line::from(""));
        }

        // Show different help text depending on workflow mode
        if self.is_in_duplicate_workflow() {
            status_lines.extend(vec![
                Line::from("Navigation: Shift+Up/Down = prev/next track | Up/Down = navigate fields | Tab = next group | Right = action pane"),
                Line::from(""),
                Line::from("Actions: F = fill to all | Ctrl+U = clear | Enter = edit/commit | Esc = exit"),
            ]);
        } else {
            status_lines.extend(vec![
                Line::from("Navigation: Tab/Shift+Tab = next/prev track | Left/Right = toggle name/value | Up/Down = navigate fields | Enter = edit/commit"),
                Line::from(""),
                Line::from("Actions: F = fill to all | Ctrl+U = clear | Esc = exit | Tab past last track to save all"),
            ]);
        }

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

    let popup_area = centered_rect(70, 20, area);

    // Clear the area first to prevent bleed-through
    f.render_widget(Clear, popup_area);

    // Modal box with opaque background
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title("Save Changes?")
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Black));

    let inner = modal_block.inner(popup_area);
    f.render_widget(modal_block, popup_area);

    // Build button spans for horizontal layout
    let buttons = [
        ("[ Save ]", 0),
        ("[ Save & Next ]", 1),
        ("[ Return ]", 2),
    ];

    let mut button_spans: Vec<Span> = Vec::new();
    for (i, (label, idx)) in buttons.iter().enumerate() {
        let style = if *idx == selected_button {
            Style::default()
                .bg(Color::Cyan)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        button_spans.push(Span::styled(*label, style));
        if i < buttons.len() - 1 {
            button_spans.push(Span::raw("  "));
        }
    }

    let lines: Vec<Line> = vec![
        Line::from(""),
        Line::from("All edits will be written to files."),
        Line::from(""),
        Line::from(button_spans),
        Line::from(""),
        Line::from("←→ Select | Enter Confirm | Esc Cancel")
            .style(Style::default().fg(Color::DarkGray)),
    ];

    let paragraph = Paragraph::new(lines)
        .alignment(Alignment::Center)
        .style(Style::default().bg(Color::Black));
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

    // Clear the area first to prevent bleed-through
    f.render_widget(Clear, modal_area);

    // Modal box with opaque background
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title("Review Changes Before Saving")
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Black));

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

    let paragraph = Paragraph::new(visible_lines).style(Style::default().bg(Color::Black));
    f.render_widget(paragraph, inner);
}
