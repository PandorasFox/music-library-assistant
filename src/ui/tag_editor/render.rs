//! Tag Editor Rendering
//!
//! All rendering methods for the unified tag editor.

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use crate::ui::widgets::{PaneConfig, ThreePaneLayout};

use super::mutations::compute_changes;
use super::state::UnifiedTagEditorState;
use super::types::{
    AggregatedTagField, AggregatedValue, FieldEditState,
    TagEditContext, TagEditorButton, TagEditorSource, UnifiedTagEditorFocus,
    UnifiedTagEditorModal, UnsavedChangesButton,
};

impl UnifiedTagEditorState {
    /// Render the unified tag editor.
    pub fn render(&mut self, f: &mut Frame, area: Rect) {
        // Layout: info pane | 3-column | status box
        let editor_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5),  // Info pane
                Constraint::Min(15),    // 3-column area
            ])
            .split(area);

        self.render_info_pane(f, editor_layout[0]);
        self.render_three_column(f, editor_layout[1]);

        // Render modal overlay if active
        if let Some(modal) = &self.modal {
            self.render_modal(f, area, modal);
        }
    }

    fn render_info_pane(&self, f: &mut Frame, area: Rect) {
        let (path, file_type, file_size, duration_ms, bitrate, sample_rate) = match &self.context {
            TagEditContext::SingleFile { audio_file, .. } => (
                audio_file.path().to_string(),
                audio_file.audio.file_type.clone(),
                audio_file.entry.file_size,
                audio_file.audio.duration_ms,
                audio_file.audio.bitrate_kbps,
                audio_file.audio.sample_rate,
            ),
            TagEditContext::BulkEdit { audio_files, .. } => {
                if let Some(audio_file) = audio_files.get(self.current_item_idx) {
                    (
                        audio_file.path().to_string(),
                        audio_file.audio.file_type.clone(),
                        audio_file.entry.file_size,
                        audio_file.audio.duration_ms,
                        audio_file.audio.bitrate_kbps,
                        audio_file.audio.sample_rate,
                    )
                } else {
                    return;
                }
            }
        };

        let duration_str = duration_ms
            .map(|ms| {
                let total_seconds = ms / 1000;
                format!("{}:{:02}", total_seconds / 60, total_seconds % 60)
            })
            .unwrap_or_else(|| "Unknown".to_string());

        let size_str = if file_size < 1024 {
            format!("{} B", file_size)
        } else if file_size < 1024 * 1024 {
            format!("{:.1} KB", file_size as f64 / 1024.0)
        } else {
            format!("{:.2} MB", file_size as f64 / (1024.0 * 1024.0))
        };

        let bitrate_str = bitrate
            .map(|b| format!("{} kbps", b))
            .unwrap_or_else(|| "Unknown".to_string());

        let sample_rate_str = sample_rate
            .map(|sr| format!("{} Hz", sr))
            .unwrap_or_else(|| "Unknown".to_string());

        let mut info_lines = vec![
            Line::from(format!("Path: {}", path)),
            Line::from(format!(
                "Format: {} | Size: {} | Duration: {} | Bitrate: {} | Sample Rate: {}",
                file_type.to_uppercase(),
                size_str,
                duration_str,
                bitrate_str,
                sample_rate_str
            )),
        ];

        // Add MP3 warning if applicable
        let is_mp3 = file_type.to_lowercase() == "mp3";
        if is_mp3 {
            info_lines.push(Line::from(
                Span::styled(
                    "Warning: MP3 files use ID3v2.3 tags with limited field support. Some tag writes may fail.",
                    Style::default().fg(Color::Rgb(255, 140, 0)) // Orange
                )
            ));
        }

        let title = format!(
            "File Info [{}/{}]",
            self.current_item_idx + 1,
            self.total_items
        );

        let info_para = Paragraph::new(info_lines)
            .block(Block::default().borders(Borders::ALL).title(title));
        f.render_widget(info_para, area);
    }

    fn render_three_column(&mut self, f: &mut Frame, area: Rect) {
        let layout = ThreePaneLayout::horizontal()
            .left(PaneConfig::new("", 30))
            .middle(PaneConfig::new("", 55))
            .right(PaneConfig::new("", 15))
            .build(area);

        self.render_context_list(f, layout.left.area);
        self.render_tag_fields_pane(f, layout.middle.area);
        self.render_action_panel(f, layout.right.area);
    }

    fn render_context_list(&mut self, f: &mut Frame, area: Rect) {
        // ContextList is display-only (not focusable), so border is never highlighted

        let (items, title): (Vec<Line>, &str) = if self.is_aggregated_mode() {
            // Aggregated mode - show single summary entry (no individual track navigation)
            let count = self.total_items;
            let summary = format!(">> {} tracks", count);
            let lines = vec![
                Line::from(summary).style(Style::default().bg(Color::DarkGray)),
                Line::from(""),
                Line::from("(bulk edit)").style(Style::default().fg(Color::DarkGray)),
            ];
            (lines, "Selection")
        } else {
            // Standard file-based rendering (Individual mode)
            let items: Vec<Line> = match &self.context {
                TagEditContext::SingleFile { audio_file, group_context, .. } => {
                    // Single file mode - show the filename
                    let filename = std::path::Path::new(audio_file.path())
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("Unknown");
                    let line = format!(">> {}", filename);

                    let mut lines = vec![Line::from(line).style(Style::default().bg(Color::DarkGray))];

                    // Show group context if present
                    if let Some(gc) = group_context {
                        lines.push(Line::from(""));
                        lines.push(Line::from(format!(
                            "Group {}/{}",
                            gc.group_index + 1,
                            gc.total_groups
                        )).style(Style::default().fg(Color::DarkGray)));
                    }

                    lines
                }
                TagEditContext::BulkEdit { audio_files, .. } => {
                    // Bulk mode with Individual editing - show all files by filename
                    // Compute visible height and scroll offset for centered selection
                    let visible_height = area.height.saturating_sub(2) as usize;
                    self.context_list_visible_height = visible_height;
                    let total = audio_files.len();

                    let ideal = self.current_item_idx.saturating_sub(visible_height / 2);
                    let max_offset = total.saturating_sub(visible_height);
                    self.context_list_scroll_offset = ideal.min(max_offset);

                    audio_files
                        .iter()
                        .enumerate()
                        .skip(self.context_list_scroll_offset)
                        .take(visible_height)
                        .map(|(idx, audio_file)| {
                            let has_changes = self.item_has_changes(idx);
                            let is_current = idx == self.current_item_idx;

                            let prefix = if is_current {
                                ">> "
                            } else if has_changes {
                                "✎ "
                            } else {
                                "   "
                            };

                            let filename = std::path::Path::new(audio_file.path())
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("Unknown");
                            let line = format!("{}{}", prefix, filename);

                            let style = if is_current && has_changes {
                                Style::default().bg(Color::DarkGray).fg(Color::Blue)
                            } else if is_current {
                                Style::default().bg(Color::DarkGray)
                            } else if has_changes {
                                Style::default().fg(Color::Blue)
                            } else {
                                Style::default()
                            };
                            Line::from(line).style(style)
                        })
                        .collect()
                }
            };

            let title = match &self.context {
                TagEditContext::SingleFile { source, .. } => match source {
                    TagEditorSource::CorpusBrowser => "File",
                    TagEditorSource::DirectoryEdit => "File",
                    TagEditorSource::TagSearch => "Search Result",
                },
                TagEditContext::BulkEdit { source, .. } => match source {
                    TagEditorSource::DirectoryEdit => "Directory",
                    _ => "Files",
                },
            };
            (items, title)
        };

        let list_para = Paragraph::new(items).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title),
        );
        f.render_widget(list_para, area);
    }

    fn render_tag_fields_pane(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, UnifiedTagEditorFocus::TagFields);

        // Check if we should render aggregated fields (directory edit mode)
        if let Some(ref agg_fields) = self.aggregated_fields {
            self.render_aggregated_fields_pane(f, area, is_focused, agg_fields.clone());
            return;
        }

        let fields = match self.tag_fields.get(self.current_item_idx) {
            Some(f) => f,
            None => return,
        };

        let original_fields = self.original_tag_fields.get(self.current_item_idx);

        // Calculate visible height
        let visible_height = area.height.saturating_sub(2) as usize;
        self.field_visible_height = visible_height;

        // Build field lines
        let field_lines: Vec<Line> = fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                // Check if modified: field is unmodified if an original (name, value) pair exists
                // This correctly handles multi-value tags (e.g., multiple Genre entries)
                let is_modified = original_fields
                    .map(|orig| {
                        !orig.iter().any(|o| {
                            o.name.eq_ignore_ascii_case(&field.name)
                                && o.value == field.value
                                && !o.deleted
                        })
                    })
                    .unwrap_or(true);

                let name_display = if is_current && matches!(self.field_edit_state, FieldEditState::EditingName) {
                    format!("{}▌", self.name_buffer)
                } else {
                    field.name.clone()
                };

                // Show indicator: deleted (✗), modified (✎), or none
                let name_with_indicator = if field.deleted {
                    format!("✗ {}", name_display)
                } else if is_modified {
                    format!("✎ {}", name_display)
                } else {
                    format!("  {}", name_display)
                };

                // Check if this field is part of a multi-value group
                let normalized_name = field.name.to_lowercase();
                let value_count = fields
                    .iter()
                    .filter(|f| f.name.to_lowercase() == normalized_name && f.name != "New Tag")
                    .count();
                let is_first_of_group = fields
                    .iter()
                    .position(|f| f.name.to_lowercase() == normalized_name)
                    .map(|pos| pos == idx)
                    .unwrap_or(true);

                let value_display = if is_current && matches!(self.field_edit_state, FieldEditState::EditingValue) {
                    format!("{}▌", self.value_buffer)
                } else if value_count > 1 && is_first_of_group {
                    // First occurrence of a multi-value tag - show count
                    format!("[{} values]", value_count)
                } else if value_count > 1 {
                    // Subsequent occurrence - show value with indent
                    format!("  └ {}", field.value)
                } else {
                    field.value.clone()
                };

                // Pad name to fixed width for alignment
                let name_padded = format!("{:18}", name_with_indicator);

                // Determine styles for name and value separately
                let (mut name_style, mut value_style) = if is_current {
                    // When current row, highlight the focused part (name or value)
                    let base = Style::default().add_modifier(Modifier::BOLD);
                    if self.focus_on_value {
                        // Value is focused - highlight value, dim name
                        (
                            base.bg(Color::DarkGray),
                            base.bg(Color::Cyan).fg(Color::Black),
                        )
                    } else {
                        // Name is focused - highlight name, dim value
                        (
                            base.bg(Color::Cyan).fg(Color::Black),
                            base.bg(Color::DarkGray),
                        )
                    }
                } else if field.deleted {
                    // Deleted fields shown in red
                    let del_style = Style::default().fg(Color::Red);
                    (del_style, del_style)
                } else if is_modified {
                    let mod_style = Style::default().fg(Color::Yellow);
                    (mod_style, mod_style)
                } else {
                    (Style::default(), Style::default())
                };

                // Apply strikethrough for deleted fields
                if field.deleted {
                    name_style = name_style.add_modifier(Modifier::CROSSED_OUT);
                    value_style = value_style.add_modifier(Modifier::CROSSED_OUT);
                }

                Line::from(vec![
                    Span::styled(name_padded, name_style),
                    Span::raw(" : "),
                    Span::styled(value_display, value_style),
                ])
            })
            .collect();

        // Scroll indicator
        let total_fields = field_lines.len();
        let scroll_indicator = if total_fields > visible_height {
            let pos = self.field_scroll_offset + 1;
            let max = total_fields.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        // Apply scroll
        let visible_lines: Vec<Line> = field_lines
            .into_iter()
            .skip(self.field_scroll_offset)
            .take(visible_height)
            .collect();

        let border_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        let tag_para = Paragraph::new(visible_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!("Tag Fields{}", scroll_indicator))
                .border_style(border_style),
        );
        f.render_widget(tag_para, area);
    }

    /// Render aggregated fields for directory-level editing.
    /// Shows whether each tag is consistent across all files or varies.
    fn render_aggregated_fields_pane(
        &mut self,
        f: &mut Frame,
        area: Rect,
        is_focused: bool,
        agg_fields: Vec<AggregatedTagField>,
    ) {
        let visible_height = area.height.saturating_sub(2) as usize;
        self.field_visible_height = visible_height;

        let field_lines: Vec<Line> = agg_fields
            .iter()
            .enumerate()
            .map(|(idx, field)| {
                let is_current = idx == self.current_field_idx;

                // Check if modified
                let is_modified = field.value != field.original_value;

                let name_display = if is_current
                    && matches!(self.field_edit_state, FieldEditState::EditingName)
                {
                    format!("{}▌", self.name_buffer)
                } else {
                    field.name.clone()
                };

                // Show indicator: modified (✎) or none
                let name_with_indicator = if is_modified {
                    format!("✎ {}", name_display)
                } else {
                    format!("  {}", name_display)
                };

                // Value display depends on AggregatedValue state
                let value_display = if is_current
                    && matches!(self.field_edit_state, FieldEditState::EditingValue)
                {
                    format!("{}▌", self.value_buffer)
                } else {
                    match &field.value {
                        AggregatedValue::Consistent(v) => {
                            if v.is_empty() {
                                "(empty)".to_string()
                            } else {
                                v.clone()
                            }
                        }
                        AggregatedValue::Various => "(various values)".to_string(),
                        AggregatedValue::VariousConfirming => "(press Enter to edit all)".to_string(),
                        AggregatedValue::Edited(v) => format!("→ {}", v),
                    }
                };

                // Pad name to fixed width for alignment
                let name_padded = format!("{:18}", name_with_indicator);

                // Determine styles
                let (name_style, value_style) = if is_current {
                    let base = Style::default().add_modifier(Modifier::BOLD);
                    if self.focus_on_value {
                        (
                            base.bg(Color::DarkGray),
                            base.bg(Color::Cyan).fg(Color::Black),
                        )
                    } else {
                        (
                            base.bg(Color::Cyan).fg(Color::Black),
                            base.bg(Color::DarkGray),
                        )
                    }
                } else if is_modified {
                    let mod_style = Style::default().fg(Color::Yellow);
                    (mod_style, mod_style)
                } else if matches!(field.value, AggregatedValue::Various) {
                    // Various values shown in magenta to draw attention
                    let var_style = Style::default().fg(Color::Magenta);
                    (Style::default(), var_style)
                } else {
                    (Style::default(), Style::default())
                };

                Line::from(vec![
                    Span::styled(name_padded, name_style),
                    Span::raw(" : "),
                    Span::styled(value_display, value_style),
                ])
            })
            .collect();

        // Scroll indicator
        let total_fields = field_lines.len();
        let scroll_indicator = if total_fields > visible_height {
            let pos = self.field_scroll_offset + 1;
            let max = total_fields.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        let visible_lines: Vec<Line> = field_lines
            .into_iter()
            .skip(self.field_scroll_offset)
            .take(visible_height)
            .collect();

        let border_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        // Show track count in title for directory view
        let title = format!("Directory Tags ({} files){}", self.total_items, scroll_indicator);

        let tag_para = Paragraph::new(visible_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        );
        f.render_widget(tag_para, area);
    }

    fn render_action_panel(&self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.focus, UnifiedTagEditorFocus::Actions);
        let buttons = self.available_buttons();
        let has_current_changes = self.has_changes_for_current_item();
        let has_anything = self.staged_decision_count > 0 || has_current_changes;

        let mut lines = vec![Line::from("")];

        for button in &buttons {
            let is_selected = *button == self.selected_button;
            let label = match button {
                TagEditorButton::ReviewAll => "Review All",
                TagEditorButton::RevertThisFile => "Revert This File",
                TagEditorButton::FillFromDisk => "Fill from Disk",
                TagEditorButton::FillFromDb => "Fill from DB",
            };

            let style = match button {
                TagEditorButton::ReviewAll => {
                    if is_focused && is_selected {
                        if has_anything {
                            Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Black).bg(Color::DarkGray).add_modifier(Modifier::BOLD)
                        }
                    } else if is_selected {
                        if has_anything { Style::default().fg(Color::Green) } else { Style::default().fg(Color::DarkGray) }
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }
                }
                TagEditorButton::RevertThisFile => {
                    if is_focused && is_selected {
                        if has_current_changes {
                            Style::default().fg(Color::Black).bg(Color::Red).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(Color::Black).bg(Color::DarkGray).add_modifier(Modifier::BOLD)
                        }
                    } else if is_selected {
                        if has_current_changes { Style::default().fg(Color::Red) } else { Style::default().fg(Color::DarkGray) }
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }
                }
                _ => {
                    // FillFromDisk, FillFromDb: keep existing uniform style
                    if is_focused && is_selected {
                        Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
                    } else if is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    }
                }
            };

            let text = if is_selected {
                format!("[ {} ]", label)
            } else {
                format!("  {}  ", label)
            };

            lines.push(Line::from(text).style(style));
        }

        // Change count
        let change_count = compute_changes(&self.original_tag_fields, &self.tag_fields).len();
        lines.push(Line::from(""));
        lines.push(
            Line::from(format!("{} changes", change_count))
                .style(Style::default().fg(Color::DarkGray)),
        );

        let border_style = if is_focused {
            Style::default().fg(Color::Green)
        } else {
            Style::default()
        };

        let action_para = Paragraph::new(lines)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("Actions")
                    .border_style(border_style),
            )
            .alignment(Alignment::Center);
        f.render_widget(action_para, area);
    }

    fn render_modal(&self, f: &mut Frame, area: Rect, modal: &UnifiedTagEditorModal) {
        match modal {
            UnifiedTagEditorModal::UnsavedChanges { selected_button } => {
                self.render_unsaved_changes_modal(f, area, *selected_button);
            }
            UnifiedTagEditorModal::MultiValueEditor {
                field_idx,
                values,
                current_value_idx,
                editing,
                edit_buffer,
            } => {
                self.render_multi_value_editor_modal(
                    f, area, *field_idx, values, *current_value_idx, *editing, edit_buffer,
                );
            }
        }
    }

    fn render_unsaved_changes_modal(
        &self,
        f: &mut Frame,
        area: Rect,
        selected_button: UnsavedChangesButton,
    ) {
        let modal_area = crate::ui::helpers::centered_rect_fixed(60, 12, area);
        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .title("Unsaved Changes")
            .border_style(Style::default().fg(Color::Yellow))
            .style(Style::default().bg(Color::Black));

        let inner = modal_block.inner(modal_area);
        f.render_widget(modal_block, modal_area);

        // Style buttons based on selection state (safe option selected by default)
        let (keep_style, discard_style) = match selected_button {
            UnsavedChangesButton::KeepEditing => (
                Style::default().fg(Color::Black).bg(Color::Green),
                Style::default().fg(Color::Red),
            ),
            UnsavedChangesButton::DiscardAndProceed => (
                Style::default().fg(Color::Green),
                Style::default().fg(Color::Black).bg(Color::Red),
            ),
        };

        let lines = vec![
            Line::from(""),
            Line::from("You have unsaved changes."),
            Line::from("Discard changes and exit the editor?"),
            Line::from(""),
            Line::from(vec![
                Span::styled(" Keep Editing ", keep_style),
                Span::raw("    "),
                Span::styled(" Discard ", discard_style),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "←/→/Tab to switch  •  Enter to confirm  •  Esc to cancel",
                Style::default().fg(Color::DarkGray),
            )),
        ];

        let paragraph = Paragraph::new(lines)
            .alignment(Alignment::Center)
            .style(Style::default().bg(Color::Black));
        f.render_widget(paragraph, inner);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_multi_value_editor_modal(
        &self,
        f: &mut Frame,
        area: Rect,
        field_idx: usize,
        values: &[String],
        current_value_idx: usize,
        editing: bool,
        edit_buffer: &str,
    ) {
        // Get the field name for the title
        let field_name = self
            .tag_fields
            .get(self.current_item_idx)
            .and_then(|fields| fields.get(field_idx))
            .map(|f| f.name.as_str())
            .unwrap_or("Tag");

        // Size modal based on content
        let height = (values.len() + 5).min(15) as u16; // values + add entry + padding + controls
        let width = 50u16;
        let modal_area = crate::ui::helpers::centered_rect_fixed(width, height, area);
        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .title(format!("Edit: {}", field_name))
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Black));

        let inner = modal_block.inner(modal_area);
        f.render_widget(modal_block, modal_area);

        let mut lines = Vec::new();

        // Render each value
        for (i, value) in values.iter().enumerate() {
            let is_current = i == current_value_idx;
            let display = if is_current && editing {
                format!("  > {}▌", edit_buffer)
            } else if is_current {
                format!("  > {}", value)
            } else {
                format!("    {}", value)
            };

            let style = if is_current {
                Style::default().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            lines.push(Line::from(display).style(style));
        }

        // "+ Add value" entry
        let add_idx = values.len();
        let is_add_current = current_value_idx == add_idx;
        let add_display = if is_add_current && editing {
            format!("  > + {}▌", edit_buffer)
        } else if is_add_current {
            "  > + Add value".to_string()
        } else {
            "    + Add value".to_string()
        };
        let add_style = if is_add_current {
            Style::default().fg(Color::Green).bg(Color::DarkGray).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(add_display).style(add_style));

        // Controls hint
        lines.push(Line::from(""));
        lines.push(
            Line::from("↑↓ Navigate | Enter Edit | Del Remove | Esc Close")
                .style(Style::default().fg(Color::DarkGray)),
        );

        let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Black));
        f.render_widget(paragraph, inner);
    }
}
