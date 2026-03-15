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

use mm_ui::field_form::FieldEditMode;
use mm_ui::tag_editor_state::FocusPane;
use mm_ui::tag_set::{AggregatedState, TagSet};

use crate::widgets::{
    render_album_art_preview, render_no_art_placeholder, AlbumArtCache, AlbumArtPicker,
    ArtCacheKey, ConfirmationButton, ConfirmationModal, PaneConfig, PathField, ThreePaneLayout,
};

use super::state::UnifiedTagEditorState;
use super::types::{
    NavigationDirection, StageChangesButton, TagEditorSource,
    UnifiedTagEditorModal, UnsavedChangesButton,
};

impl UnifiedTagEditorState {
    /// Render the unified tag editor.
    pub fn render(
        &mut self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
        resolver: &mm_meta::paths::PathResolver,
    ) {
        let editor_layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(5), // Info pane
                Constraint::Min(15),   // 3-column area
            ])
            .split(area);

        self.render_info_pane(f, editor_layout[0]);
        self.render_three_column(f, editor_layout[1], art_picker, art_cache, resolver);

        if let Some(modal) = &self.modal {
            self.render_modal(f, area, modal);
        }
    }

    fn render_info_pane(&self, f: &mut Frame, area: Rect) {
        let audio_file = match self.audio_files.get(self.core.current_file) {
            Some(af) => af,
            None => return,
        };

        let path = audio_file.path().to_string();
        let file_type = audio_file.audio.file_type.clone();
        let file_size = audio_file.entry.file_size;
        let duration_ms = audio_file.audio.duration_ms;
        let bitrate = audio_file.audio.bitrate_kbps;
        let sample_rate = audio_file.audio.sample_rate;

        let duration_str = duration_ms
            .map(crate::helpers::format_duration_ms)
            .unwrap_or_else(|| "Unknown".to_string());
        let size_str = crate::helpers::format_bytes(file_size as u64);
        let bitrate_str = bitrate
            .map(crate::helpers::format_kbps)
            .unwrap_or_else(|| "Unknown".to_string());
        let sample_rate_str = sample_rate
            .map(crate::helpers::format_sample_rate)
            .unwrap_or_else(|| "Unknown".to_string());

        let inner_width = area.width.saturating_sub(2);
        let mut info_lines = PathField::new(Span::raw("Path: "), &path).render_lines(inner_width);
        info_lines.push(Line::from(format!(
            "Format: {} | Size: {} | Duration: {} | Bitrate: {} | Sample Rate: {}",
            file_type.to_uppercase(),
            size_str,
            duration_str,
            bitrate_str,
            sample_rate_str
        )));

        if file_type.to_lowercase() == "mp3" {
            info_lines.push(Line::from(Span::styled(
                "Warning: MP3 files use ID3v2.3 tags with limited field support. Some tag writes may fail.",
                Style::default().fg(Color::Rgb(255, 140, 0)),
            )));
        }

        let title = format!(
            "File Info [{}/{}]",
            self.core.current_file + 1,
            self.audio_files.len()
        );

        let info_para =
            Paragraph::new(info_lines).block(Block::default().borders(Borders::ALL).title(title));
        f.render_widget(info_para, area);
    }

    fn render_three_column(
        &mut self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
        resolver: &mm_meta::paths::PathResolver,
    ) {
        let layout = ThreePaneLayout::horizontal()
            .left(PaneConfig::new("", 30))
            .middle(PaneConfig::new("", 55))
            .right(PaneConfig::new("", 15))
            .build(area);

        self.render_context_list(f, layout.left.area);
        self.render_tag_fields_pane(f, layout.middle.area);

        let right_area = layout.right.area;
        let right_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(right_area.width.saturating_sub(2)),
                Constraint::Min(8),
            ])
            .split(right_area);

        self.render_art_preview_pane(f, right_chunks[0], art_picker, art_cache, resolver);
        self.render_action_panel(f, right_chunks[1]);
    }

    fn render_context_list(&self, f: &mut Frame, area: Rect) {
        let (items, title): (Vec<Line>, &str) = if self.is_aggregated_mode() {
            let count = self.audio_files.len();
            let summary = format!(">> {} tracks", count);
            let lines = vec![
                Line::from(summary).style(Style::default().bg(Color::DarkGray)),
                Line::from(""),
                Line::from("(bulk edit)").style(Style::default().fg(Color::DarkGray)),
            ];
            (lines, "Selection")
        } else if self.audio_files.len() == 1 {
            let filename = std::path::Path::new(self.audio_files[0].path())
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("Unknown");
            let line = format!(">> {}", filename);
            let mut lines = vec![Line::from(line).style(Style::default().bg(Color::DarkGray))];

            if let Some(ref gc) = self.group_context {
                lines.push(Line::from(""));
                lines.push(
                    Line::from(format!("Group {}/{}", gc.group_index + 1, gc.total_groups))
                        .style(Style::default().fg(Color::DarkGray)),
                );
            }

            let title = match self.source {
                TagEditorSource::CorpusBrowser | TagEditorSource::DirectoryEdit => "File",
                TagEditorSource::TagSearch => "Search Result",
                TagEditorSource::HealthModal => "Health",
            };
            (lines, title)
        } else {
            // Multi-file individual mode
            let visible_height = area.height.saturating_sub(2) as usize;
            let total = self.audio_files.len();
            let current = self.core.current_file;

            let ideal = current.saturating_sub(visible_height / 2);
            let max_offset = total.saturating_sub(visible_height);
            let scroll_offset = ideal.min(max_offset);

            let items: Vec<Line> = self.audio_files
                .iter()
                .enumerate()
                .skip(scroll_offset)
                .take(visible_height)
                .map(|(idx, audio_file)| {
                    let has_changes = self.item_has_changes(idx);
                    let is_current = idx == current;

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
                .collect();

            let title = match self.source {
                TagEditorSource::DirectoryEdit => "Directory",
                _ => "Files",
            };
            (items, title)
        };

        let list_para =
            Paragraph::new(items).block(Block::default().borders(Borders::ALL).title(title));
        f.render_widget(list_para, area);
    }

    fn render_tag_fields_pane(&mut self, f: &mut Frame, area: Rect) {
        let is_focused = matches!(self.core.focus, FocusPane::Content);
        self.fields_pane_rect = Some(area);

        if self.is_aggregated_mode() {
            if let Some(ref agg) = self.core.aggregated {
                let agg = agg.clone();
                self.render_aggregated_fields_pane(f, area, is_focused, &agg);
            }
            return;
        }

        let tag_set = match self.core.tag_sets.get(self.core.current_file) {
            Some(ts) => ts,
            None => return,
        };
        let original_set = self.core.original_tag_sets.get(self.core.current_file);

        let visible_height = area.height.saturating_sub(2) as usize;
        self.core.form.visible_height = visible_height;

        let inner_area = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        // +1 for "New Tag" sentinel
        let total = tag_set.entry_count() + 1;
        self.field_click_targets.populate(inner_area, self.core.form.scroll_offset, total);

        let field_lines = self.build_entry_lines(tag_set, original_set);

        let total_fields = field_lines.len();
        let scroll_indicator = if total_fields > visible_height {
            let pos = self.core.form.scroll_offset + 1;
            let max = total_fields.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        let visible_lines: Vec<Line> = field_lines
            .into_iter()
            .skip(self.core.form.scroll_offset)
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

    /// Build display lines from TagSet entries.
    fn build_entry_lines<'a>(
        &self,
        tag_set: &TagSet,
        original_set: Option<&TagSet>,
    ) -> Vec<Line<'a>> {
        let form = &self.core.form;
        let mut lines = Vec::new();

        for (idx, entry) in tag_set.entries().iter().enumerate() {
            let is_current = idx == form.cursor;

            // Check if modified vs original
            let is_modified = original_set
                .and_then(|orig| orig.get(idx))
                .map(|orig_entry| orig_entry != entry)
                .unwrap_or(true);

            // Name display
            let name_display =
                if is_current && form.edit_mode == FieldEditMode::EditingName {
                    let (before, cursor_ch, after) = form.name_input.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else {
                    entry.name.clone()
                };

            let name_with_indicator = if is_modified {
                format!("✎ {}", name_display)
            } else {
                format!("  {}", name_display)
            };

            // Value display
            let value_display =
                if is_current && form.edit_mode == FieldEditMode::EditingValue {
                    let (before, cursor_ch, after) = form.value_input.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else if entry.values.len() > 1 {
                    format!("[{} values]", entry.values.len())
                } else {
                    entry.values.first().cloned().unwrap_or_default()
                };

            let name_padded = format!("{:18}", name_with_indicator);

            let (name_style, value_style) = if is_current {
                let base = Style::default().add_modifier(Modifier::BOLD);
                if form.edit_mode == FieldEditMode::EditingValue {
                    (base.bg(Color::DarkGray), base.bg(Color::Cyan).fg(Color::Black))
                } else if form.edit_mode == FieldEditMode::EditingName {
                    (base.bg(Color::Cyan).fg(Color::Black), base.bg(Color::DarkGray))
                } else {
                    // Navigating — highlight the whole row
                    (base.bg(Color::DarkGray), base.bg(Color::DarkGray))
                }
            } else if is_modified {
                let mod_style = Style::default().fg(Color::Yellow);
                (mod_style, mod_style)
            } else {
                (Style::default(), Style::default())
            };

            lines.push(Line::from(vec![
                Span::styled(name_padded, name_style),
                Span::raw(" : "),
                Span::styled(value_display, value_style),
            ]));
        }

        // "New Tag" sentinel
        let is_sentinel_current = form.cursor == tag_set.entry_count();
        let sentinel_style = if is_sentinel_current {
            Style::default()
                .fg(Color::Green)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from("  + New Tag").style(sentinel_style));

        lines
    }

    /// Render aggregated fields for directory-level editing.
    fn render_aggregated_fields_pane(
        &mut self,
        f: &mut Frame,
        area: Rect,
        is_focused: bool,
        agg: &mm_ui::tag_set::AggregatedTagSet,
    ) {
        let visible_height = area.height.saturating_sub(2) as usize;
        self.core.form.visible_height = visible_height;

        let inner_area = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        let total = agg.entry_count() + 1; // +1 for sentinel
        self.field_click_targets
            .populate(inner_area, self.core.form.scroll_offset, total);

        let form = &self.core.form;
        let mut field_lines: Vec<Line> = Vec::new();

        for (idx, entry) in agg.entries().iter().enumerate() {
            let is_current = idx == form.cursor;
            let is_modified = matches!(entry.state, AggregatedState::Edited(_));

            let name_display =
                if is_current && form.edit_mode == FieldEditMode::EditingName {
                    let (before, cursor_ch, after) = form.name_input.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else {
                    entry.name.clone()
                };

            let name_with_indicator = if is_modified {
                format!("✎ {}", name_display)
            } else {
                format!("  {}", name_display)
            };

            let value_display =
                if is_current && form.edit_mode == FieldEditMode::EditingValue {
                    let (before, cursor_ch, after) = form.value_input.cursor_splits();
                    format!("{}{}{}", before, cursor_ch, after)
                } else {
                    match &entry.state {
                        AggregatedState::Consistent(vals) => {
                            if vals.is_empty() {
                                "(empty)".to_string()
                            } else {
                                vals.join(", ")
                            }
                        }
                        AggregatedState::Various => "(various values)".to_string(),
                        AggregatedState::Edited(vals) => format!("→ {}", vals.join(", ")),
                    }
                };

            let name_padded = format!("{:18}", name_with_indicator);

            let (name_style, value_style) = if is_current {
                let base = Style::default().add_modifier(Modifier::BOLD);
                if form.edit_mode == FieldEditMode::EditingValue {
                    (base.bg(Color::DarkGray), base.bg(Color::Cyan).fg(Color::Black))
                } else if form.edit_mode == FieldEditMode::EditingName {
                    (base.bg(Color::Cyan).fg(Color::Black), base.bg(Color::DarkGray))
                } else {
                    (base.bg(Color::DarkGray), base.bg(Color::DarkGray))
                }
            } else if is_modified {
                let mod_style = Style::default().fg(Color::Yellow);
                (mod_style, mod_style)
            } else if matches!(entry.state, AggregatedState::Various) {
                let var_style = Style::default().fg(Color::Magenta);
                (Style::default(), var_style)
            } else {
                (Style::default(), Style::default())
            };

            field_lines.push(Line::from(vec![
                Span::styled(name_padded, name_style),
                Span::raw(" : "),
                Span::styled(value_display, value_style),
            ]));
        }

        // Sentinel
        let is_sentinel_current = form.cursor == agg.entry_count();
        let sentinel_style = if is_sentinel_current {
            Style::default()
                .fg(Color::Green)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        field_lines.push(Line::from("  + New Tag").style(sentinel_style));

        let total_fields = field_lines.len();
        let scroll_indicator = if total_fields > visible_height {
            let pos = self.core.form.scroll_offset + 1;
            let max = total_fields.saturating_sub(visible_height) + 1;
            format!(" [{}/{}]", pos, max)
        } else {
            String::new()
        };

        let visible_lines: Vec<Line> = field_lines
            .into_iter()
            .skip(self.core.form.scroll_offset)
            .take(visible_height)
            .collect();

        let border_style = if is_focused {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default()
        };

        let title = format!(
            "Directory Tags ({} files){}",
            self.audio_files.len(),
            scroll_indicator
        );

        let tag_para = Paragraph::new(visible_lines).block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border_style),
        );
        f.render_widget(tag_para, area);
    }

    fn render_art_preview_pane(
        &self,
        f: &mut Frame,
        area: Rect,
        art_picker: &mut AlbumArtPicker,
        art_cache: &mut AlbumArtCache,
        resolver: &mm_meta::paths::PathResolver,
    ) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title("Art")
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(area);
        f.render_widget(block, area);

        if inner.width < 2 || inner.height < 2 {
            return;
        }

        let rel_path = match self.selected_path() {
            Some(p) => p.to_string(),
            None => {
                render_no_art_placeholder(f, inner);
                return;
            }
        };
        let abs_path = resolver.resolve(std::path::Path::new(&rel_path));

        let key = ArtCacheKey::Embedded(abs_path.clone());
        art_cache.retain_only_keys(&[key]);

        let cached = art_cache.get_or_load_embedded(&abs_path, art_picker);

        if cached.width == 0 {
            render_no_art_placeholder(f, inner);
        } else {
            if inner.height > 3 {
                let split = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Min(2), Constraint::Length(1)])
                    .split(inner);

                render_album_art_preview(f, split[0], cached);

                let info = format!(
                    "{}x{} {}",
                    cached.width,
                    cached.height,
                    cached.format.to_uppercase()
                );
                let info_line =
                    Line::from(Span::styled(info, Style::default().fg(Color::DarkGray)));
                f.render_widget(
                    Paragraph::new(info_line).alignment(Alignment::Center),
                    split[1],
                );
            } else {
                render_album_art_preview(f, inner, cached);
            }
        }
    }

    fn render_action_panel(&mut self, f: &mut Frame, area: Rect) {
        use mm_ui::modal_buttons::ModalButtons;
        use mm_ui::tag_editor_state::TagEditorButton;

        let is_focused = matches!(self.core.focus, FocusPane::Buttons);
        let ctx = self.core.button_ctx();

        self.actions_pane_rect = Some(area);
        self.action_click_targets.clear();
        let inner_area = Rect {
            x: area.x + 1,
            y: area.y + 1,
            width: area.width.saturating_sub(2),
            height: area.height.saturating_sub(2),
        };
        self.action_click_targets.set_list_area(inner_area);

        let buttons = TagEditorButton::all();
        for (idx, _) in buttons.iter().enumerate() {
            let row_y = inner_area.y + 1 + idx as u16;
            if row_y < inner_area.y + inner_area.height {
                self.action_click_targets.add_row(idx.to_string(), row_y);
            }
        }

        let mut lines = vec![Line::from("")];

        for &button in buttons {
            let is_selected = button == self.core.buttons.selected;

            let label = match button {
                TagEditorButton::ReviewAll => {
                    if self.is_embedded() {
                        "Save & Return"
                    } else {
                        "Review All"
                    }
                }
                TagEditorButton::Revert => "Revert",
                TagEditorButton::Cancel => "Cancel",
            };

            let enabled = button.enabled(&ctx);
            let button_color = button.color(&ctx);

            let style = if is_focused && is_selected {
                if enabled {
                    Style::default()
                        .fg(Color::Black)
                        .bg(button_color)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD)
                }
            } else if is_selected {
                if enabled {
                    Style::default().fg(button_color)
                } else {
                    Style::default().fg(Color::DarkGray)
                }
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let text = if is_selected {
                format!("[ {} ]", label)
            } else {
                format!("  {}  ", label)
            };

            lines.push(Line::from(text).style(style));
        }

        // Change count from staged decision count
        let staged = self.core.staged_decision_count;
        let has_current = self.has_changes_for_current_item();
        lines.push(Line::from(""));
        if staged > 0 || has_current {
            let count_text = if has_current && staged > 0 {
                format!("{} staged + pending", staged)
            } else if staged > 0 {
                format!("{} staged", staged)
            } else {
                "pending changes".to_string()
            };
            lines.push(
                Line::from(count_text).style(Style::default().fg(Color::Yellow)),
            );
        } else {
            lines.push(
                Line::from("no changes").style(Style::default().fg(Color::DarkGray)),
            );
        }

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
                edit_input,
            } => {
                self.render_multi_value_editor_modal(
                    f,
                    area,
                    *field_idx,
                    values,
                    *current_value_idx,
                    *editing,
                    edit_input,
                );
            }
            UnifiedTagEditorModal::StageChangesConfirm {
                direction,
                selected_button,
            } => {
                self.render_stage_changes_modal(f, area, *direction, *selected_button);
            }
        }
    }

    fn render_unsaved_changes_modal(
        &self,
        f: &mut Frame,
        area: Rect,
        selected_button: UnsavedChangesButton,
    ) {
        ConfirmationModal::new("Unsaved Changes")
            .border_color(Color::Yellow)
            .fixed_size(60, 10)
            .message(vec![
                Line::from(""),
                Line::from("You have unsaved changes."),
                Line::from("Discard changes and exit the editor?"),
                Line::from(""),
            ])
            .buttons(vec![
                ConfirmationButton::new("Keep Editing", Color::Green)
                    .selected(selected_button == UnsavedChangesButton::KeepEditing),
                ConfirmationButton::new("Discard", Color::Red)
                    .selected(selected_button == UnsavedChangesButton::DiscardAndProceed),
            ])
            .hint("←/→/Tab to switch  •  Enter to confirm  •  Esc to cancel")
            .render(f, area);
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
        edit_input: &crate::widgets::TextInputState,
    ) {
        let field_name = self
            .current_tag_set()
            .and_then(|ts: &mm_ui::tag_set::TagSet| ts.get(field_idx))
            .map(|e| e.name.as_str())
            .unwrap_or("Tag");

        let height = (values.len() + 5).min(15) as u16;
        let width = 50u16;
        let modal_area = crate::widgets::centered_rect_fixed(width, height, area);
        f.render_widget(Clear, modal_area);

        let modal_block = Block::default()
            .borders(Borders::ALL)
            .title(format!("Edit: {}", field_name))
            .border_style(Style::default().fg(Color::Cyan))
            .style(Style::default().bg(Color::Black));

        let inner = modal_block.inner(modal_area);
        f.render_widget(modal_block, modal_area);

        let mut lines = Vec::new();

        for (i, value) in values.iter().enumerate() {
            let is_current = i == current_value_idx;
            let display = if is_current && editing {
                let (before, cursor_ch, after) = edit_input.cursor_splits();
                format!("  > {}{}{}", before, cursor_ch, after)
            } else if is_current {
                format!("  > {}", value)
            } else {
                format!("    {}", value)
            };

            let style = if is_current {
                Style::default()
                    .bg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };

            lines.push(Line::from(display).style(style));
        }

        // "+ Add value" entry
        let add_idx = values.len();
        let is_add_current = current_value_idx == add_idx;
        let add_display = if is_add_current && editing {
            let (before, cursor_ch, after) = edit_input.cursor_splits();
            format!("  > + {}{}{}", before, cursor_ch, after)
        } else if is_add_current {
            "  > + Add value".to_string()
        } else {
            "    + Add value".to_string()
        };
        let add_style = if is_add_current {
            Style::default()
                .fg(Color::Green)
                .bg(Color::DarkGray)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Green)
        };
        lines.push(Line::from(add_display).style(add_style));

        lines.push(Line::from(""));
        lines.push(
            Line::from("↑↓ Navigate | Enter Edit | Del Remove | Esc Close")
                .style(Style::default().fg(Color::DarkGray)),
        );

        let paragraph = Paragraph::new(lines).style(Style::default().bg(Color::Black));
        f.render_widget(paragraph, inner);
    }

    fn render_stage_changes_modal(
        &self,
        f: &mut Frame,
        area: Rect,
        direction: NavigationDirection,
        selected_button: StageChangesButton,
    ) {
        let filename = self.current_item_label();
        let direction_label = match direction {
            NavigationDirection::Next => "next",
            NavigationDirection::Prev => "previous",
        };

        ConfirmationModal::new("Stage Changes?")
            .border_color(Color::Yellow)
            .fixed_size(50, 10)
            .message(vec![
                Line::from(""),
                Line::from(format!("Stage changes for {}?", filename)),
                Line::from(""),
            ])
            .buttons(vec![
                ConfirmationButton::new("Yes", Color::Green)
                    .selected(selected_button == StageChangesButton::Yes),
                ConfirmationButton::new("No", Color::Yellow)
                    .selected(selected_button == StageChangesButton::No),
                ConfirmationButton::new("Cancel", Color::White)
                    .selected(selected_button == StageChangesButton::Cancel),
            ])
            .hint(format!(
                "Yes: stage & go {}  No: skip & go {}  Cancel: stay",
                direction_label, direction_label
            ))
            .render(f, area);
    }
}
