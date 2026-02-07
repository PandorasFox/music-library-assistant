//! Compound Split V2 Rendering
//!
//! Renders the three-pane compound tag split resolution interface:
//! - Title bar with group indicator and mode (safe/review)
//! - Left pane (25%): Split parts with exists/new indicators
//! - Middle pane (35%): Files with selection checkboxes
//! - Right pane (40%): Tag values for selected file
//! - Edit field (in review mode when editing)
//! - Controls hint bar

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::{render_pane, truncate_right};

use super::types::{CompoundSplitStateV2, FocusPaneV2};

/// Render the three-pane compound split resolution interface.
pub fn render(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    // Clear background
    f.render_widget(Clear, area);

    // Layout depends on whether we're editing
    let chunks = if state.is_editing() {
        Layout::vertical([
            Constraint::Length(3), // Title bar
            Constraint::Min(8),    // Three-pane content
            Constraint::Length(3), // Edit input
            Constraint::Length(2), // Controls
        ])
        .split(area)
    } else {
        Layout::vertical([
            Constraint::Length(3), // Title bar
            Constraint::Min(8),    // Three-pane content
            Constraint::Length(2), // Controls
        ])
        .split(area)
    };

    render_title(f, chunks[0], state);
    render_three_panes(f, chunks[1], state);

    if state.is_editing() {
        render_edit_input(f, chunks[2], state);
        render_controls(f, chunks[3], state);
    } else {
        render_controls(f, chunks[2], state);
    }
}

/// Render the title bar.
fn render_title(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let group_indicator = format!("({}/{})", state.group_index + 1, state.total_groups);
    let mode_label = if state.is_safe_mode { "Safe" } else { "Review" };

    let title = format!(
        " Split \"{}\" compound {} - {} ",
        state.data.compound.tag_name, group_indicator, mode_label
    );

    let border_color = if state.is_safe_mode {
        Color::Green
    } else {
        Color::Yellow
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    f.render_widget(block, area);
}

/// Render the three content panes.
fn render_three_panes(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let panes = Layout::horizontal([
        Constraint::Percentage(25), // Parts
        Constraint::Percentage(35), // Files
        Constraint::Percentage(40), // Tag details
    ])
    .split(area);

    render_parts_pane(f, panes[0], state);
    render_files_pane(f, panes[1], state);
    render_tags_pane(f, panes[2], state);
}

/// Render the split parts pane (left).
fn render_parts_pane(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let is_focused = state.focus_pane == FocusPaneV2::Parts && !state.is_editing();
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let title = format!(" SPLIT PARTS ({}) ", state.edited_parts.len());

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = render_pane(f, area, block);

    let visible_height = inner.height as usize;
    let max_width = inner.width.saturating_sub(1) as usize;

    // Calculate scroll
    let scroll = if state.part_cursor >= state.part_scroll + visible_height {
        state.part_cursor.saturating_sub(visible_height - 1)
    } else if state.part_cursor < state.part_scroll {
        state.part_cursor
    } else {
        state.part_scroll
    };

    let items: Vec<ListItem> = state
        .edited_parts
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, part)| {
            let is_cursor = i == state.part_cursor;
            let exists = state.data.compound.part_exists(part);

            // Indicator for exists/new
            let indicator = if exists { "[ok]" } else { "[new]" };
            let indicator_color = if exists { Color::Green } else { Color::Yellow };

            // Truncate part to fit
            let part_max = max_width.saturating_sub(indicator.len() + 3);
            let display_part = truncate_right(part, part_max);

            let mut spans = vec![
                Span::styled(
                    format!("{} ", indicator),
                    Style::default().fg(indicator_color),
                ),
            ];

            let text_style = if is_cursor && is_focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            spans.push(Span::styled(display_part, text_style));

            ListItem::new(Line::from(spans))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Render the files pane (middle) with selection checkboxes.
fn render_files_pane(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let is_focused = state.focus_pane == FocusPaneV2::Files && !state.is_editing();
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let selected_count = state.selected_files.len();
    let total_count = state.data.files.len();
    let title = format!(" TRACKS ({}/{}) ", selected_count, total_count);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = render_pane(f, area, block);

    let visible_height = inner.height as usize;
    let max_width = inner.width.saturating_sub(1) as usize;

    // Calculate scroll
    let scroll = if state.file_cursor >= state.file_scroll + visible_height {
        state.file_cursor.saturating_sub(visible_height - 1)
    } else if state.file_cursor < state.file_scroll {
        state.file_cursor
    } else {
        state.file_scroll
    };

    let items: Vec<ListItem> = state
        .data
        .files
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, file)| {
            let is_cursor = i == state.file_cursor;
            let selected = state.selected_files.contains(&i);

            let checkbox = if selected { "[x]" } else { "[ ]" };

            // Truncate filename to fit
            let name_max = max_width.saturating_sub(checkbox.len() + 2);
            let display_name = truncate_right(&file.filename, name_max);

            let text = format!("{} {}", checkbox, display_name);

            let style = if is_cursor && is_focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Render the tag values pane (right).
fn render_tags_pane(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let block = Block::default()
        .title(" TAG VALUES ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = render_pane(f, area, block);

    // Show tags for the selected file
    let file = state.data.files.get(state.file_cursor);

    let items: Vec<ListItem> = match file {
        Some(f) => {
            let max_width = inner.width.saturating_sub(1) as usize;

            f.tag_values
                .iter()
                .map(|(name, value)| {
                    let text = format!("{}: {}", name, value);
                    let display = truncate_right(&text, max_width);

                    // Highlight the tag being split
                    let style = if name.to_lowercase() == state.data.compound.tag_name.to_lowercase()
                        && value == &state.data.compound.compound_value
                    {
                        Style::default().fg(Color::Yellow)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    };

                    ListItem::new(Line::from(Span::styled(display, style)))
                })
                .collect()
        }
        None => vec![ListItem::new(Line::from(Span::styled(
            "(no file selected)",
            Style::default().fg(Color::DarkGray),
        )))],
    };

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Render the edit input field (only shown when editing).
fn render_edit_input(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let block = Block::default()
        .title(" Edit part ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Render the text input
    let input_text = state.part_input.value();
    let cursor_pos = state.part_input.cursor;

    // Show cursor
    let (before, after) = input_text.split_at(cursor_pos.min(input_text.len()));
    let cursor_char = after.chars().next().unwrap_or(' ');
    let after_cursor = if after.len() > 1 { &after[cursor_char.len_utf8()..] } else { "" };

    let line = Line::from(vec![
        Span::styled(before, Style::default().fg(Color::White)),
        Span::styled(
            cursor_char.to_string(),
            Style::default()
                .fg(Color::Black)
                .bg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(after_cursor, Style::default().fg(Color::White)),
    ]);

    let para = Paragraph::new(line);
    f.render_widget(para, inner);
}

/// Render the controls hint bar.
fn render_controls(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let mut hints = Vec::new();

    if state.is_editing() {
        // Editing mode controls
        hints.extend([
            Span::styled("[Enter]", Style::default().fg(Color::Green)),
            Span::raw(" Save  "),
            Span::styled("[Esc]", Style::default().fg(Color::Red)),
            Span::raw(" Cancel"),
        ]);
    } else {
        // Normal mode controls
        hints.extend([
            Span::styled("[^/v]", Style::default().fg(Color::Cyan)),
            Span::raw(" nav  "),
            Span::styled("[</>]", Style::default().fg(Color::Cyan)),
            Span::raw(" pane  "),
        ]);

        if state.focus_pane == FocusPaneV2::Files {
            hints.extend([
                Span::styled("[Space]", Style::default().fg(Color::Magenta)),
                Span::raw(" toggle  "),
            ]);
        }

        if !state.is_safe_mode && state.focus_pane == FocusPaneV2::Parts {
            hints.extend([
                Span::styled("[E]", Style::default().fg(Color::Yellow)),
                Span::raw(" edit  "),
            ]);
        }

        hints.extend([
            Span::styled("[Enter]", Style::default().fg(Color::Green)),
            Span::raw(" split  "),
            Span::styled("[^Q]", Style::default().fg(Color::Yellow)),
            Span::raw(" keep  "),
            Span::styled("[^R]", Style::default().fg(Color::Blue)),
            Span::raw(" review  "),
            Span::styled("[Esc]", Style::default().fg(Color::Red)),
            Span::raw(" cancel"),
        ]);
    }

    let para = Paragraph::new(Line::from(hints)).alignment(Alignment::Center);
    f.render_widget(para, area);
}
