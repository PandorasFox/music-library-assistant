//! Directory Tag Editor Rendering
//!
//! All UI rendering functions for the directory tag editor.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph},
    Frame,
};

use crate::ui::widgets::{PaneConfig, ThreePaneLayout};

use super::directory_state::{DirectoryTagChange, DirectoryTagEditorState};
use super::types::{AggregatedValue, DirectoryTagEditorFocus, VariousConfirmState};

impl DirectoryTagEditorState {
    /// Render the directory tag editor
    pub fn render(&mut self, f: &mut Frame, area: Rect, status_message: Option<&str>) {
        // If gathering, show progress popup
        if self.is_gathering() {
            self.render_gathering_popup(f, area);
            return;
        }

        // Layout: info pane | 3-column | status box
        let editor_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),  // Info pane
                Constraint::Min(15),    // 3-column area
                Constraint::Length(4),  // Status box
            ])
            .split(area);

        self.render_info_pane(f, editor_layout[0]);
        self.render_three_column(f, editor_layout[1]);
        self.render_status_box(f, editor_layout[2], status_message);
    }

    fn render_gathering_popup(&self, f: &mut Frame, area: Rect) {
        use super::super::helpers::centered_rect;

        let popup_area = centered_rect(50, 20, area);

        // Clear and draw popup
        f.render_widget(Clear, popup_area);

        let gathering = self.gathering_state.as_ref().unwrap();
        let progress_text = format!(
            "Gathering metadata: {}/{}",
            gathering.processed_files,
            if gathering.total_files > 0 {
                gathering.total_files.to_string()
            } else {
                "...".to_string()
            }
        );

        let current_file = gathering
            .current_file
            .as_ref()
            .map(|f| crate::ui::helpers::truncate_left(f, 40))
            .unwrap_or_default();

        let lines = vec![
            Line::from(""),
            Line::from(progress_text).style(Style::default().add_modifier(Modifier::BOLD)),
            Line::from(""),
            Line::from(current_file).style(Style::default().fg(Color::DarkGray)),
            Line::from(""),
            Line::from("Press Esc to cancel").style(Style::default().fg(Color::Yellow)),
        ];

        let popup = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Loading Directory")
                    .border_style(Style::default().fg(Color::Cyan))
                    .style(Style::default().bg(Color::Black)),
            )
            .alignment(Alignment::Center)
            .style(Style::default().bg(Color::Black));

        f.render_widget(popup, popup_area);
    }

    fn render_info_pane(&self, f: &mut Frame, area: Rect) {
        // Split into left (info) and right (sibling spinner) panes
        let panes = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Percentage(65), // Directory info
                Constraint::Percentage(35), // Sibling spinner
            ])
            .split(area);

        self.render_directory_info(f, panes[0]);
        self.render_sibling_spinner(f, panes[1]);
    }

    fn render_directory_info(&self, f: &mut Frame, area: Rect) {
        let dir_name = self
            .current_directory
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| self.current_directory.to_string_lossy().to_string());

        // Count file extensions
        let mut ext_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for file in &self.files {
            if let Some(ext) = file.path.extension() {
                *ext_counts.entry(ext.to_string_lossy().to_lowercase()).or_default() += 1;
            }
        }
        let mut exts: Vec<_> = ext_counts.into_iter().collect();
        exts.sort_by(|a, b| b.1.cmp(&a.1));
        let ext_str = exts
            .iter()
            .map(|(ext, count)| format!("{} ({})", ext, count))
            .collect::<Vec<_>>()
            .join(", ");

        let info_lines = vec![
            Line::from(format!("Path: {}", self.current_directory.display())),
            Line::from(format!(
                "Files: {} | Formats: {}",
                self.files.len(),
                if ext_str.is_empty() { "none" } else { &ext_str }
            )),
        ];

        let info_para = Paragraph::new(info_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Directory - {}", dir_name)),
        );
        f.render_widget(info_para, area);
    }

    fn render_sibling_spinner(&self, f: &mut Frame, area: Rect) {
        // Get previous, current, and next directory names
        let prev_name = if self.current_sibling_idx > 0 {
            self.sibling_directories
                .get(self.current_sibling_idx - 1)
                .and_then(|p| p.file_name())
                .map(|n| format!("   {}", n.to_string_lossy()))
                .unwrap_or_default()
        } else {
            String::new()
        };

        let current_name = self
            .current_directory
            .file_name()
            .map(|n| format!("-> {}", n.to_string_lossy()))
            .unwrap_or_else(|| "-> (unknown)".to_string());

        let next_name = if self.current_sibling_idx + 1 < self.sibling_directories.len() {
            self.sibling_directories
                .get(self.current_sibling_idx + 1)
                .and_then(|p| p.file_name())
                .map(|n| format!("   {}", n.to_string_lossy()))
                .unwrap_or_default()
        } else {
            String::new()
        };

        let lines = vec![
            Line::from(prev_name).style(Style::default().fg(Color::DarkGray)),
            Line::from(current_name).style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
            Line::from(next_name).style(Style::default().fg(Color::DarkGray)),
        ];

        let position_info = format!(
            "{}/{}",
            self.current_sibling_idx + 1,
            self.sibling_directories.len()
        );

        let spinner_para = Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Siblings [{}]", position_info)),
        );
        f.render_widget(spinner_para, area);
    }

    fn render_three_column(&mut self, f: &mut Frame, area: Rect) {
        // Use widget for consistent three-pane layout
        let layout = ThreePaneLayout::horizontal()
            .left(PaneConfig::new("", 30))
            .middle(PaneConfig::new("", 60))
            .right(PaneConfig::new("", 10))
            .build(area);

        self.render_file_list(f, layout.left.area);
        self.render_tag_fields(f, layout.middle.area);
        self.render_action_panel(f, layout.right.area);
    }

    fn render_file_list(&mut self, f: &mut Frame, area: Rect) {
        self.file_visible_height = area.height.saturating_sub(2) as usize;

        let file_items: Vec<ListItem> = self
            .files
            .iter()
            .enumerate()
            .skip(self.file_scroll_offset)
            .take(self.file_visible_height)
            .map(|(idx, file)| {
                let style = if idx == self.selected_file_idx {
                    Style::default().bg(Color::DarkGray)
                } else {
                    Style::default()
                };
                ListItem::new(Line::from(file.filename.clone())).style(style)
            })
            .collect();

        let scroll_indicator = if self.files.len() > self.file_visible_height {
            let pos = self.file_scroll_offset + 1;
            let max = self.files.len().saturating_sub(self.file_visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        let file_list = List::new(file_items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Files{}", scroll_indicator)),
        );
        f.render_widget(file_list, area);
    }

    fn render_tag_fields(&mut self, f: &mut Frame, area: Rect) {
        let visible_height = area.height.saturating_sub(2) as usize;
        self.tag_visible_height = visible_height;

        let field_lines: Vec<Line> = self
            .aggregated_fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                // Check if modified from original
                let is_modified = self
                    .original_aggregated_fields
                    .get(idx)
                    .map(|orig| orig.value != field.value)
                    .unwrap_or(true);

                // Build value display
                let value_display = match &field.value {
                    AggregatedValue::Consistent(s) => {
                        if is_current && self.field_edit_state != super::types::FieldEditState::NonEditable {
                            format!("{}_", self.value_buffer)
                        } else {
                            s.clone()
                        }
                    }
                    AggregatedValue::Various => "(various values)".to_string(),
                    AggregatedValue::VariousConfirming => {
                        if matches!(self.various_confirm_state, Some(VariousConfirmState::Editing)) {
                            format!("{}_", self.value_buffer)
                        } else {
                            "(overwrite all? press Enter)".to_string()
                        }
                    }
                    AggregatedValue::Edited(s) => {
                        if is_current && self.field_edit_state != super::types::FieldEditState::NonEditable {
                            format!("{}_", self.value_buffer)
                        } else {
                            s.clone()
                        }
                    }
                };

                // Modification indicator
                let name_with_indicator = if is_modified && field.name != "New Tag" {
                    format!("* {}", field.name)
                } else {
                    format!("  {}", field.name)
                };

                let line_text = format!("{:18} : {}", name_with_indicator, value_display);

                // Style based on state
                let style = if is_current {
                    Style::default()
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                } else if matches!(field.value, AggregatedValue::Various) {
                    Style::default().fg(Color::DarkGray)
                } else if is_modified {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };

                Line::from(line_text).style(style)
            })
            .collect();

        // Apply scroll offset
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

        let tag_para = Paragraph::new(visible_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Aggregated Tags{}", scroll_indicator)),
        );
        f.render_widget(tag_para, area);
    }

    fn render_action_panel(&self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, DirectoryTagEditorFocus::ActionPane);

        let changes = self.compute_changes();
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
        } else {
            status_lines.push(Line::from(
                "Tab/Shift+Tab = prev/next directory | Right = action | Enter = edit | Esc = exit",
            ));
        }

        let status_para = Paragraph::new(status_lines)
            .block(Block::default().borders(Borders::ALL).title("Status"));
        f.render_widget(status_para, area);
    }
}

// ============================================================================
// Modal Rendering
// ============================================================================

/// Render the change preview modal for directory editor
pub fn render_directory_change_preview_modal(
    f: &mut Frame,
    area: Rect,
    changes: &[DirectoryTagChange],
    total_files: usize,
    scroll_offset: usize,
) {
    use super::super::helpers::centered_rect;

    let modal_area = centered_rect(70, 70, area);

    f.render_widget(Clear, modal_area);

    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title("Review Changes Before Saving")
        .border_style(Style::default().fg(Color::Yellow))
        .style(Style::default().bg(Color::Black));

    let inner = modal_block.inner(modal_area);
    f.render_widget(modal_block, modal_area);

    let mut lines = Vec::new();

    lines.push(
        Line::from(format!(
            "Changes will be applied to {} files",
            total_files
        ))
        .style(
            Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::Cyan),
        ),
    );
    lines.push(Line::from(""));

    for change in changes {
        let line_text = format!(
            "  {}: '{}'",
            change.field_name,
            if change.new_value.is_empty() {
                "(clear)"
            } else {
                &change.new_value
            }
        );
        lines.push(Line::from(line_text).style(Style::default().fg(Color::White)));
    }

    lines.push(Line::from(""));
    lines.push(
        Line::from("Press Enter/Y to confirm, Esc/N to cancel")
            .style(Style::default().fg(Color::DarkGray)),
    );

    // Apply scroll
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

/// Render the unsaved changes prompt
pub fn render_unsaved_changes_modal(f: &mut Frame, area: Rect, going_next: bool) {
    use super::super::helpers::centered_rect;

    let popup_area = centered_rect(50, 20, area);

    f.render_widget(Clear, popup_area);

    let direction = if going_next { "next" } else { "previous" };

    let lines = vec![
        Line::from(""),
        Line::from("You have unsaved changes.").style(Style::default().add_modifier(Modifier::BOLD)),
        Line::from(""),
        Line::from(format!("Switch to {} directory?", direction)),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                "[S]ave & Switch",
                Style::default().fg(Color::Green).add_modifier(Modifier::BOLD),
            ),
            Span::raw("  "),
            Span::styled(
                "[D]iscard & Switch",
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("  "),
            Span::styled("[C]ancel", Style::default().fg(Color::Red)),
        ]),
    ];

    let popup = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title("Unsaved Changes")
                .border_style(Style::default().fg(Color::Yellow))
                .style(Style::default().bg(Color::Black)),
        )
        .alignment(Alignment::Center)
        .style(Style::default().bg(Color::Black));

    f.render_widget(popup, popup_area);
}
