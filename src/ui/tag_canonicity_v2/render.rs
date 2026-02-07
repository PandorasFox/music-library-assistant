//! Tag Canonicity V2 Rendering
//!
//! Renders the three-pane tag canonicity resolution interface:
//! - Title bar with group indicator
//! - Left pane (25%): Variant values with checkboxes
//! - Middle pane (35%): File list (navigable)
//! - Right pane (40%): All tag values for selected file (informational)
//! - Text input for canonical value
//! - Controls hint bar

use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::{render_pane, truncate_right};

use super::types::{FocusPaneV2, TagCanonicalityStateV2};

/// Render the three-pane tag canonicity resolution interface.
pub fn render(f: &mut Frame, area: Rect, state: &TagCanonicalityStateV2) {
    // Clear background
    f.render_widget(Clear, area);

    // Layout: title (3) + input (3) + content (min) + controls (2)
    // Input at top so up/down navigation matches visual layout (cursor -1 = top)
    let chunks = Layout::vertical([
        Constraint::Length(3), // Title bar
        Constraint::Length(3), // Text input (at top)
        Constraint::Min(8),    // Three-pane content
        Constraint::Length(2), // Controls
    ])
    .split(area);

    render_title(f, chunks[0], state);
    render_input(f, chunks[1], state);
    render_three_panes(f, chunks[2], state);
    render_controls(f, chunks[3]);
}

/// Render the title bar.
fn render_title(f: &mut Frame, area: Rect, state: &TagCanonicalityStateV2) {
    let group_indicator = format!("({}/{})", state.group_index + 1, state.total_groups);

    let title = if let Some(ref context) = state.data.context_label {
        format!(
            " Set \"{}\" variants {} - {} ",
            state.data.tag_name, group_indicator, context
        )
    } else {
        format!(
            " Squash \"{}\" variants {} ",
            state.data.tag_name, group_indicator
        )
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    f.render_widget(block, area);
}

/// Render the three content panes.
fn render_three_panes(f: &mut Frame, area: Rect, state: &TagCanonicalityStateV2) {
    let panes = Layout::horizontal([
        Constraint::Percentage(25), // Variants
        Constraint::Percentage(35), // Files
        Constraint::Percentage(40), // Tag details
    ])
    .split(area);

    render_variants_pane(f, panes[0], state);
    render_files_pane(f, panes[1], state);
    render_tags_pane(f, panes[2], state);
}

/// Render the variants pane (left).
fn render_variants_pane(f: &mut Frame, area: Rect, state: &TagCanonicalityStateV2) {
    let is_focused = state.focus_pane == FocusPaneV2::Variants;
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let selected_count = state.selected_variants.len();
    let total_count = state.data.variants.len();
    let title = format!(" VALUES ({}/{}) ", selected_count, total_count);

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = render_pane(f, area, block);

    let visible_height = inner.height as usize;
    let max_width = inner.width.saturating_sub(1) as usize; // Leave space for cursor

    // Calculate scroll to keep cursor visible
    let cursor_pos = state.variant_cursor.max(0) as usize;
    let scroll = if cursor_pos >= state.variant_scroll + visible_height {
        cursor_pos.saturating_sub(visible_height - 1)
    } else if cursor_pos < state.variant_scroll {
        cursor_pos
    } else {
        state.variant_scroll
    };

    let items: Vec<ListItem> = state
        .data
        .variants
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
        .map(|(i, variant)| {
            let selected = state.selected_variants.contains(&i);
            let is_cursor = state.variant_cursor >= 0 && i == state.variant_cursor as usize;

            let checkbox = if selected { "[x]" } else { "[ ]" };

            // Truncate variant value to fit
            let value_max = max_width.saturating_sub(checkbox.len() + 6); // " (N)"
            let display_value = truncate_right(&variant.value, value_max);

            let text = format!("{} {} ({})", checkbox, display_value, variant.count);

            let style = if is_cursor && is_focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(Line::from(Span::styled(text, style)))
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Render the files pane (middle).
fn render_files_pane(f: &mut Frame, area: Rect, state: &TagCanonicalityStateV2) {
    let is_focused = state.focus_pane == FocusPaneV2::Files;
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };

    let title = format!(" TRACKS ({}) ", state.data.files.len());

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));

    let inner = render_pane(f, area, block);

    let visible_height = inner.height as usize;
    let max_width = inner.width.saturating_sub(2) as usize;

    // Calculate scroll to keep cursor visible
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
            let cursor_indicator = if is_cursor { "> " } else { "  " };

            // Truncate filename to fit
            let filename_max = max_width.saturating_sub(cursor_indicator.len());
            let display_name = truncate_right(&file.filename, filename_max);

            let style = if is_cursor && is_focused {
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else if is_cursor {
                // Cursor but not focused
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::White)
            };

            let line = Line::from(vec![
                Span::raw(cursor_indicator),
                Span::styled(display_name, style),
            ]);

            ListItem::new(line)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Render the tags pane (right) - shows all tags for selected file.
fn render_tags_pane(f: &mut Frame, area: Rect, state: &TagCanonicalityStateV2) {
    let block = Block::default()
        .title(" TAG VALUES ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = render_pane(f, area, block);

    // Get selected file's tags
    let Some(file) = state.data.files.get(state.file_cursor) else {
        return;
    };

    let max_width = inner.width.saturating_sub(1) as usize;
    let visible_height = inner.height as usize;

    // Sort tags by name for consistent display
    let mut sorted_tags: Vec<_> = file.tag_values.iter().collect();
    sorted_tags.sort_by(|a, b| a.0.cmp(&b.0));

    let lines: Vec<Line> = sorted_tags
        .iter()
        .take(visible_height)
        .map(|(tag_name, tag_value)| {
            // Highlight the tag being canonicalized
            let is_target_tag = tag_name.eq_ignore_ascii_case(&state.data.tag_name);

            let name_style = if is_target_tag {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let value_style = if is_target_tag {
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            // Truncate value to fit
            let prefix_len = tag_name.len() + 2; // "tag: "
            let value_max = max_width.saturating_sub(prefix_len);
            let display_value = truncate_right(tag_value, value_max);

            Line::from(vec![
                Span::styled(format!("{}: ", tag_name), name_style),
                Span::styled(display_value, value_style),
            ])
        })
        .collect();

    let para = Paragraph::new(lines);
    f.render_widget(para, inner);
}

/// Render the canonical value input field.
fn render_input(f: &mut Frame, area: Rect, state: &TagCanonicalityStateV2) {
    let is_focused =
        state.focus_pane == FocusPaneV2::Variants && state.variant_cursor == -1;
    // Magenta when unfocused = editable but not currently focused
    // Cyan when focused = active input
    let input_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::Magenta)
    };

    let label = if state.pre_filled {
        " Squash to: "
    } else {
        " Album artist: "
    };

    let block = Block::default()
        .title(label)
        .borders(Borders::ALL)
        .border_style(input_style);

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Render text with cursor at proper position
    let value = state.canonical_input.value();
    let cursor_pos = state.canonical_input.cursor;

    if is_focused {
        // Split text at cursor and render with cursor character
        let before: String = value.chars().take(cursor_pos).collect();
        let cursor_char = value.chars().nth(cursor_pos);
        let after: String = value.chars().skip(cursor_pos + 1).collect();

        let spans = if let Some(c) = cursor_char {
            vec![
                Span::styled(before, Style::default().fg(Color::White)),
                Span::styled(
                    c.to_string(),
                    Style::default().fg(Color::Black).bg(Color::White),
                ),
                Span::styled(after, Style::default().fg(Color::White)),
            ]
        } else {
            // Cursor at end - show block cursor
            vec![
                Span::styled(before, Style::default().fg(Color::White)),
                Span::styled(" ", Style::default().fg(Color::Black).bg(Color::White)),
            ]
        };

        let input = Paragraph::new(Line::from(spans));
        f.render_widget(input, inner);
    } else {
        // Not focused - just render text
        let input = Paragraph::new(value).style(Style::default().fg(Color::White));
        f.render_widget(input, inner);
    }
}

/// Render the controls hint bar.
fn render_controls(f: &mut Frame, area: Rect) {
    let hints = "[^/v] navigate  [</> pane]  [Space] toggle  [F] fill  [Enter] confirm  [Tab] next  [^R] review  [Esc] cancel";

    let hint = Paragraph::new(hints)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);

    f.render_widget(hint, area);
}
