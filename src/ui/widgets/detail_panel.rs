//! Detail Panel Widget
//!
//! A reusable right-side panel for on-demand info/detail views.
//! Renders fields (booleans, string lists, text) with cursor highlighting,
//! buttons at the bottom, and a hint line.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use super::selection_styles::CURSOR_STYLE;

/// A field to display in the detail panel.
pub struct DetailField<'a> {
    pub label: &'a str,
    pub widget: DetailWidget<'a>,
}

/// Widget type for a detail field.
pub enum DetailWidget<'a> {
    /// Optional boolean: None = "(inherit)", Some(v) = "true"/"false".
    /// Cycles None → Some(true) → Some(false) → None on toggle.
    OptBool { value: Option<bool>, edited: bool },
    /// Vertical list of strings with cursor, add/edit/delete.
    StringItems {
        items: &'a [String],
        cursor: Option<usize>,
        edited: bool,
    },
    /// Single-line text value, editable with Enter.
    Text { value: &'a str, edited: bool },
}

/// A button at the bottom of the panel.
pub struct PanelButton<'a> {
    pub label: &'a str,
    pub color: Color,
    pub selected: bool,
}

/// Renders a bordered detail panel with fields, buttons, and hint line.
pub fn render_detail_panel(
    f: &mut Frame,
    area: Rect,
    title: &str,
    border_color: Color,
    fields: &[DetailField],
    field_cursor: usize,
    buttons: &[PanelButton],
    focus_on_buttons: bool,
    hint: Option<&str>,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(title.to_string());

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 3 || inner.width < 10 {
        return;
    }

    // Reserve space: bottom 2 lines for buttons + hint
    let button_height = if buttons.is_empty() { 0 } else { 1 };
    let hint_height = if hint.is_some() { 1 } else { 0 };
    let footer_height = button_height + hint_height;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),
            Constraint::Length(footer_height),
        ])
        .split(inner);

    let fields_area = chunks[0];
    let footer_area = chunks[1];

    // Render fields
    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in fields.iter().enumerate() {
        let is_focused = !focus_on_buttons && i == field_cursor;
        let label_style = if is_focused {
            Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Cyan)
        };

        match &field.widget {
            DetailWidget::OptBool { value, edited } => {
                let val_str = match value {
                    Some(true) => "true",
                    Some(false) => "false",
                    None => "(inherit)",
                };
                let val_style = if is_focused {
                    CURSOR_STYLE
                } else if *edited {
                    Style::default().fg(Color::Green)
                } else if value.is_none() {
                    Style::default().fg(Color::DarkGray)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", field.label), label_style),
                    Span::styled(val_str.to_string(), val_style),
                ]));
            }
            DetailWidget::Text { value, edited } => {
                let val_style = if is_focused {
                    CURSOR_STYLE
                } else if *edited {
                    Style::default().fg(Color::Green)
                } else {
                    Style::default().fg(Color::White)
                };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}: ", field.label), label_style),
                    Span::styled(value.to_string(), val_style),
                ]));
            }
            DetailWidget::StringItems { items, cursor, edited } => {
                let edit_marker = if *edited { " *" } else { "" };
                lines.push(Line::from(vec![
                    Span::styled(format!("{}{}:", field.label, edit_marker), label_style),
                ]));
                if items.is_empty() {
                    let empty_style = if is_focused {
                        Style::default().fg(Color::DarkGray).add_modifier(Modifier::ITALIC)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };
                    lines.push(Line::from(vec![
                        Span::raw("  "),
                        Span::styled("(none)", empty_style),
                    ]));
                } else {
                    for (j, item) in items.iter().enumerate() {
                        let item_focused = is_focused && *cursor == Some(j);
                        let item_style = if item_focused {
                            CURSOR_STYLE
                        } else {
                            Style::default().fg(Color::White)
                        };
                        let indicator = if item_focused { "▶ " } else { "  " };
                        lines.push(Line::from(vec![
                            Span::raw("  "),
                            Span::styled(format!("{}{}", indicator, item), item_style),
                        ]));
                    }
                }
            }
        }

        // Add spacing between fields
        if i + 1 < fields.len() {
            lines.push(Line::raw(""));
        }
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, fields_area);

    // Render footer (buttons + hint)
    if footer_height > 0 {
        let footer_chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(
                if button_height > 0 && hint_height > 0 {
                    vec![Constraint::Length(1), Constraint::Length(1)]
                } else if button_height > 0 {
                    vec![Constraint::Length(1)]
                } else {
                    vec![Constraint::Length(1)]
                },
            )
            .split(footer_area);

        let mut chunk_idx = 0;

        // Buttons
        if !buttons.is_empty() {
            let button_spans: Vec<Span> = buttons
                .iter()
                .enumerate()
                .flat_map(|(i, btn)| {
                    let style = if focus_on_buttons && btn.selected {
                        Style::default()
                            .fg(Color::Black)
                            .bg(btn.color)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(btn.color)
                    };
                    let mut spans = vec![Span::styled(format!("[{}]", btn.label), style)];
                    if i + 1 < buttons.len() {
                        spans.push(Span::raw("  "));
                    }
                    spans
                })
                .collect();
            f.render_widget(Paragraph::new(Line::from(button_spans)), footer_chunks[chunk_idx]);
            chunk_idx += 1;
        }

        // Hint
        if let Some(hint_text) = hint {
            if chunk_idx < footer_chunks.len() {
                let hint_line = Line::styled(
                    hint_text.to_string(),
                    Style::default().fg(Color::DarkGray),
                );
                f.render_widget(Paragraph::new(hint_line), footer_chunks[chunk_idx]);
            }
        }
    }
}
