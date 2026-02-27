//! Config Editor Rendering
//!
//! Full-screen layout with scrollable field list and Save/Discard buttons.
//! Collection fields (StringSet, StringPairMap, StringListMap) render inline
//! with expandable item lists when editing.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ui::widgets::control_colors;
use super::state::{ConfigEditorState, EditorButton, EditorFocus, CollectionPosition};
use super::types::{ConfigField, ConfigValue, FieldSource};

/// Render the config editor view into the given content area.
pub fn render(f: &mut Frame, area: Rect, state: &ConfigEditorState) {
    // Layout: field list (fills) + button row (1 line) + hints (1 line)
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),     // Scrollable field list
            Constraint::Length(1),  // Button row
            Constraint::Length(1),  // Control hints
        ])
        .split(area);

    render_field_list(f, chunks[0], state);
    render_buttons(f, chunks[1], state);
    render_hints(f, chunks[2], state);
}

/// Render the scrollable field list with group headers.
fn render_field_list(f: &mut Frame, area: Rect, state: &ConfigEditorState) {
    let visible_height = area.height as usize;
    let inner_width = area.width as usize;

    // Build flat list of renderable lines (group headers + fields + collection items)
    let mut lines: Vec<(Line<'_>, bool)> = Vec::new(); // (line, is_scroll_target)
    let mut flat_idx: usize = 0;

    for group in &state.groups {
        // Group header
        let collapse_indicator = if group.collapsed { "\u{25b8}" } else { "\u{25be}" };
        let header_style = Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD);
        lines.push((
            Line::from(vec![
                Span::styled(format!(" {} ", collapse_indicator), header_style),
                Span::styled(group.name, header_style),
            ]),
            false,
        ));

        if group.collapsed {
            lines.push((
                Line::from(Span::styled(
                    "   (collapsed — Shift+C to expand)",
                    Style::default().fg(Color::DarkGray),
                )),
                false,
            ));
            continue;
        }

        for field in &group.fields {
            let is_cursor = flat_idx == state.cursor;
            flat_idx += 1;

            // Check if this field is a collection being edited inline
            let is_collection_expanded = is_cursor && state.collection_pos.is_some();

            // Render field header line
            let line = render_field_line(field, is_cursor, inner_width, state);
            // If collection is expanded, scroll target is the selected item, not the header
            lines.push((line, is_cursor && !is_collection_expanded));

            // If expanded collection, render items inline
            if is_collection_expanded {
                let collection_lines = render_collection_items(field, state, inner_width);
                lines.extend(collection_lines);
            }
        }

        // Blank separator between groups
        lines.push((Line::from(""), false));
    }

    // Auto-scroll to keep target line visible
    let target_line_idx = lines.iter().position(|(_, is_target)| *is_target).unwrap_or(0);
    let scroll = if target_line_idx < state.scroll_offset {
        target_line_idx
    } else if target_line_idx >= state.scroll_offset + visible_height {
        target_line_idx.saturating_sub(visible_height) + 1
    } else {
        state.scroll_offset
    };

    // Render visible lines
    let visible_lines: Vec<Line<'_>> = lines.into_iter()
        .skip(scroll)
        .take(visible_height)
        .map(|(line, _)| line)
        .collect();

    let paragraph = Paragraph::new(visible_lines);
    f.render_widget(paragraph, area);
}

/// Render a single field line.
fn render_field_line<'a>(
    field: &'a super::types::ConfigField,
    is_cursor: bool,
    _width: usize,
    state: &ConfigEditorState,
) -> Line<'a> {
    let cursor_indicator = if is_cursor && state.focus == EditorFocus::Fields {
        "> "
    } else {
        "  "
    };

    // If text input is active on this row (not in collection/sub-collection), show the input
    if is_cursor && state.text_input.is_some() && state.collection_pos.is_none() {
        let input = state.text_input.as_ref().unwrap();
        return Line::from(vec![
            Span::styled(cursor_indicator, Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
            Span::raw(format!("{:<38}", field.label)),
            Span::styled(
                format!("{}\u{2588}", input.value),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
            ),
        ]);
    }

    let value_color = match field.source {
        FieldSource::Default => Color::DarkGray,
        FieldSource::Loaded => Color::Blue,
        FieldSource::Edited => Color::Magenta,
    };

    let label_style = if is_cursor && state.focus == EditorFocus::Fields {
        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let source_tag = match field.source {
        FieldSource::Default => "(default)",
        FieldSource::Loaded => "(loaded)",
        FieldSource::Edited => "(edited)",
    };

    let restart_tag = if field.restart_required { " (restart)" } else { "" };

    let value_str = field.value.display();

    Line::from(vec![
        Span::styled(cursor_indicator.to_string(), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled(format!("{:<38}", field.label), label_style),
        Span::styled(format!("{:<24}", value_str), Style::default().fg(value_color)),
        Span::styled(source_tag, Style::default().fg(Color::DarkGray)),
        Span::styled(restart_tag, Style::default().fg(Color::DarkGray)),
    ])
}

/// Render inline collection items when a collection field is expanded.
fn render_collection_items<'a>(
    field: &'a ConfigField,
    state: &ConfigEditorState,
    _width: usize,
) -> Vec<(Line<'a>, bool)> {
    let mut lines = Vec::new();
    let pos = state.collection_pos;
    let text_input = state.text_input.as_ref();
    let pair_focus = state.pair_field_focus;

    match &field.value {
        ConfigValue::StringSet(items) => {
            for (i, item) in items.iter().enumerate() {
                let is_selected = pos == Some(CollectionPosition::Item(i));
                let is_editing = is_selected && text_input.is_some();
                let is_target = is_selected;

                let cursor_char = if is_selected { "    > " } else { "      " };

                let line = if is_editing {
                    let input = text_input.unwrap();
                    Line::from(vec![
                        Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                        Span::styled(
                            format!("{}\u{2588}", input.value),
                            Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
                        ),
                    ])
                } else {
                    let style = if is_selected {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    Line::from(vec![
                        Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                        Span::styled(item.as_str(), style),
                    ])
                };
                lines.push((line, is_target));
            }
            // Add new row
            let is_add_new = pos == Some(CollectionPosition::AddNew);
            let is_editing = is_add_new && text_input.is_some();
            let cursor_char = if is_add_new { "    > " } else { "      " };

            let line = if is_editing {
                let input = text_input.unwrap();
                Line::from(vec![
                    Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                    Span::styled(
                        format!("{}\u{2588}", input.value),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
                    ),
                ])
            } else {
                let style = if is_add_new {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                Line::from(vec![
                    Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                    Span::styled("[+] Add new...", style),
                ])
            };
            lines.push((line, is_add_new));
        }

        ConfigValue::StringPairMap(items) => {
            for (i, (key, value)) in items.iter().enumerate() {
                let is_selected = pos == Some(CollectionPosition::Item(i));
                let is_editing = is_selected && text_input.is_some();
                let is_target = is_selected;

                let cursor_char = if is_selected { "    > " } else { "      " };

                let line = if is_editing {
                    let input = text_input.unwrap();
                    let (key_display, value_display) = if pair_focus == 0 {
                        (format!("{}\u{2588}", input.value), value.clone())
                    } else {
                        (key.clone(), format!("{}\u{2588}", input.value))
                    };
                    let key_style = if pair_focus == 0 {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let value_style = if pair_focus == 1 {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED)
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    Line::from(vec![
                        Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                        Span::styled(key_display, key_style),
                        Span::styled(" \u{2192} ", Style::default().fg(Color::DarkGray)),
                        Span::styled(value_display, value_style),
                    ])
                } else {
                    let key_style = if is_selected && pair_focus == 0 {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    let value_style = if is_selected && pair_focus == 1 {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::Cyan)
                    };
                    Line::from(vec![
                        Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                        Span::styled(key.as_str(), key_style),
                        Span::styled(" \u{2192} ", Style::default().fg(Color::DarkGray)),
                        Span::styled(value.as_str(), value_style),
                    ])
                };
                lines.push((line, is_target));
            }
            // Add new row
            let is_add_new = pos == Some(CollectionPosition::AddNew);
            let is_editing = is_add_new && text_input.is_some();
            let cursor_char = if is_add_new { "    > " } else { "      " };

            let line = if is_editing {
                let input = text_input.unwrap();
                Line::from(vec![
                    Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                    Span::styled(
                        format!("{}\u{2588}", input.value),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
                    ),
                    Span::styled(" \u{2192} ", Style::default().fg(Color::DarkGray)),
                    Span::styled("...", Style::default().fg(Color::DarkGray)),
                ])
            } else {
                let style = if is_add_new {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                Line::from(vec![
                    Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                    Span::styled("[+] Add new...", style),
                ])
            };
            lines.push((line, is_add_new));
        }

        ConfigValue::StringListMap(items) => {
            let sub_pos = state.sub_collection_pos;

            for (i, (tag, separators)) in items.iter().enumerate() {
                let is_selected = pos == Some(CollectionPosition::Item(i));
                let is_expanded = is_selected && sub_pos.is_some();
                // Scroll target is this line only if selected but NOT expanded
                // (when expanded, the target is within the sub-list)
                let is_target = is_selected && !is_expanded;

                let cursor_char = if is_selected { "    > " } else { "      " };

                if is_expanded {
                    // Show tag header with colon, no separator summary
                    let tag_style = Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD);
                    lines.push((
                        Line::from(vec![
                            Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                            Span::styled(tag.as_str(), tag_style),
                            Span::styled(":", Style::default().fg(Color::DarkGray)),
                        ]),
                        false,
                    ));

                    // Render each separator as its own indented line
                    for (si, sep) in separators.iter().enumerate() {
                        let is_sub_selected = sub_pos == Some(CollectionPosition::Item(si));
                        let is_sub_editing = is_sub_selected && text_input.is_some();
                        let sub_cursor = if is_sub_selected { "        > " } else { "          " };

                        let line = if is_sub_editing {
                            let input = text_input.unwrap();
                            Line::from(vec![
                                Span::styled(sub_cursor, Style::default().fg(Color::Yellow)),
                                Span::styled(
                                    format!("{}\u{2588}", input.value),
                                    Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
                                ),
                            ])
                        } else {
                            let style = if is_sub_selected {
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                            } else {
                                Style::default().fg(Color::Cyan)
                            };
                            Line::from(vec![
                                Span::styled(sub_cursor, Style::default().fg(Color::Yellow)),
                                Span::styled(format!("\"{}\"", sep), style),
                            ])
                        };
                        lines.push((line, is_sub_selected));
                    }

                    // Sub-collection add-new row
                    let is_sub_add = sub_pos == Some(CollectionPosition::AddNew);
                    let is_sub_editing = is_sub_add && text_input.is_some();
                    let sub_cursor = if is_sub_add { "        > " } else { "          " };

                    let line = if is_sub_editing {
                        let input = text_input.unwrap();
                        Line::from(vec![
                            Span::styled(sub_cursor, Style::default().fg(Color::Yellow)),
                            Span::styled(
                                format!("{}\u{2588}", input.value),
                                Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
                            ),
                        ])
                    } else {
                        let style = if is_sub_add {
                            Style::default().fg(Color::Green)
                        } else {
                            Style::default().fg(Color::DarkGray)
                        };
                        Line::from(vec![
                            Span::styled(sub_cursor, Style::default().fg(Color::Yellow)),
                            Span::styled("[+] Add new...", style),
                        ])
                    };
                    lines.push((line, is_sub_add));
                } else {
                    // Collapsed: show tag with separator summary on one line
                    let seps_display = if separators.is_empty() {
                        "(no separators)".to_string()
                    } else {
                        separators.iter().map(|s| format!("\"{}\"", s)).collect::<Vec<_>>().join(", ")
                    };

                    let tag_style = if is_selected {
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(Color::White)
                    };
                    lines.push((
                        Line::from(vec![
                            Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                            Span::styled(tag.as_str(), tag_style),
                            Span::styled(": ", Style::default().fg(Color::DarkGray)),
                            Span::styled(seps_display, Style::default().fg(Color::Cyan)),
                        ]),
                        is_target,
                    ));
                }
            }
            // Add new tag row
            let is_add_new = pos == Some(CollectionPosition::AddNew);
            let is_editing = is_add_new && text_input.is_some();
            let cursor_char = if is_add_new { "    > " } else { "      " };

            let line = if is_editing {
                let input = text_input.unwrap();
                Line::from(vec![
                    Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                    Span::styled(
                        format!("{}\u{2588}", input.value),
                        Style::default().fg(Color::Yellow).add_modifier(Modifier::UNDERLINED),
                    ),
                    Span::styled(": (new tag)", Style::default().fg(Color::DarkGray)),
                ])
            } else {
                let style = if is_add_new {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                Line::from(vec![
                    Span::styled(cursor_char, Style::default().fg(Color::Yellow)),
                    Span::styled("[+] Add new tag...", style),
                ])
            };
            lines.push((line, is_add_new));
        }

        _ => {}
    }

    lines
}

/// Render the Save / Discard button row.
fn render_buttons(f: &mut Frame, area: Rect, state: &ConfigEditorState) {
    let in_buttons = state.focus == EditorFocus::Buttons;

    let save_style = if in_buttons && state.selected_button == EditorButton::Save {
        Style::default().fg(Color::Black).bg(Color::Green).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Green)
    };

    let discard_style = if in_buttons && state.selected_button == EditorButton::Discard {
        Style::default().fg(Color::Black).bg(Color::Gray).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let has_edits = state.has_edits();
    let save_label = if has_edits { " [ Save ] " } else { " [ Save ] " };

    let line = Line::from(vec![
        Span::raw("                                       "),
        Span::styled(save_label, save_style),
        Span::raw("   "),
        Span::styled(" [ Discard ] ", discard_style),
    ]);

    let paragraph = Paragraph::new(line);
    f.render_widget(paragraph, area);
}

/// Render the control hints line.
fn render_hints(f: &mut Frame, area: Rect, state: &ConfigEditorState) {
    let hints = if state.text_input.is_some() {
        Line::from(vec![
            control_colors::confirm("Enter"),
            control_colors::text(" accept  "),
            control_colors::cancel("Esc"),
            control_colors::text(" cancel"),
        ])
    } else if state.focus == EditorFocus::Buttons {
        Line::from(vec![
            control_colors::nav("</>"),
            control_colors::text(" switch  "),
            control_colors::confirm("Enter"),
            control_colors::text(" select  "),
            control_colors::cancel("Esc"),
            control_colors::text(" back"),
        ])
    } else if state.sub_collection_pos.is_some() {
        // Sub-collection (separator) editing mode hints
        Line::from(vec![
            control_colors::nav("^v"),
            control_colors::text(" nav  "),
            control_colors::confirm("Enter"),
            control_colors::text(" edit  "),
            control_colors::cancel("x"),
            control_colors::text(" delete  "),
            control_colors::cancel("Esc"),
            control_colors::text(" back"),
        ])
    } else if state.collection_pos.is_some() {
        // Collection editing mode hints
        let is_pair_map = state.cursor_to_group_field()
            .map(|(gi, fi)| matches!(&state.groups[gi].fields[fi].value, ConfigValue::StringPairMap(_)))
            .unwrap_or(false);

        if is_pair_map {
            Line::from(vec![
                control_colors::nav("^v"),
                control_colors::text(" nav  "),
                control_colors::nav("</>"),
                control_colors::text(" field  "),
                control_colors::confirm("Enter"),
                control_colors::text(" edit  "),
                control_colors::cancel("x"),
                control_colors::text(" del  "),
                control_colors::cancel("Esc"),
                control_colors::text(" back"),
            ])
        } else {
            Line::from(vec![
                control_colors::nav("^v"),
                control_colors::text(" nav  "),
                control_colors::confirm("Enter"),
                control_colors::text(" edit  "),
                control_colors::cancel("x"),
                control_colors::text(" delete  "),
                control_colors::cancel("Esc"),
                control_colors::text(" back"),
            ])
        }
    } else {
        // Show field description if cursor is on a field
        let description = state.cursor_to_group_field()
            .map(|(gi, fi)| state.groups[gi].fields[fi].description)
            .unwrap_or("");

        if !description.is_empty() {
            Line::from(vec![
                control_colors::nav("^v"),
                control_colors::text(" nav  "),
                control_colors::confirm("Enter"),
                control_colors::text(" edit  "),
                control_colors::edit("r"),
                control_colors::text(" reset  "),
                control_colors::cancel("Esc"),
                control_colors::text(" discard  "),
                Span::styled(
                    format!("\u{2502} {}", description),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        } else {
            Line::from(vec![
                control_colors::nav("^v"),
                control_colors::text(" nav  "),
                control_colors::nav("[/]"),
                control_colors::text(" group  "),
                control_colors::confirm("Enter"),
                control_colors::text(" edit  "),
                control_colors::edit("r"),
                control_colors::text(" reset  "),
                control_colors::toggle("S+v"),
                control_colors::text(" buttons  "),
                control_colors::cancel("Esc"),
                control_colors::text(" discard"),
            ])
        }
    };

    let paragraph = Paragraph::new(hints);
    f.render_widget(paragraph, area);
}
