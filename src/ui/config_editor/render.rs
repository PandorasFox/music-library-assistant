//! Config Editor Rendering
//!
//! Full-screen layout with scrollable field list and Save/Discard buttons.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use crate::ui::widgets::control_colors;
use super::state::{ConfigEditorState, EditorButton, EditorFocus};
use super::types::FieldSource;

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

    // Build flat list of renderable lines (group headers + fields)
    let mut lines: Vec<(Line<'_>, bool)> = Vec::new(); // (line, is_cursor_row)
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

            let line = render_field_line(field, is_cursor, inner_width, state);
            lines.push((line, is_cursor));
        }

        // Blank separator between groups
        lines.push((Line::from(""), false));
    }

    // Auto-scroll to keep cursor visible
    let cursor_line_idx = lines.iter().position(|(_, is_cursor)| *is_cursor).unwrap_or(0);
    let scroll = if cursor_line_idx < state.scroll_offset {
        cursor_line_idx
    } else if cursor_line_idx >= state.scroll_offset + visible_height {
        cursor_line_idx.saturating_sub(visible_height) + 1
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

    // If text input is active on this row, show the input
    if is_cursor && state.text_input.is_some() {
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
