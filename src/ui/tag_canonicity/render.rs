//! Tag Canonicity Modal Rendering
//!
//! Renders the tag canonicity resolution modal with:
//! - Editable canonical value text field AT TOP
//! - Variant list with toggle-select checkboxes BELOW
//! - Optional confirmation modal overlay

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::ui::widgets::centered_rect_fixed;

use super::types::{TagCanonicalityModal, TagCanonicalityState};

/// Render the tag canonicity resolution modal.
pub fn render(f: &mut Frame, area: Rect, state: &TagCanonicalityState) {
    // Calculate modal size - wider to fit long album names with counts
    // Use centered_rect_fixed which properly accounts for area.x/area.y offsets
    let modal_width = 72.min(area.width.saturating_sub(4));
    let modal_height = 20.min(area.height.saturating_sub(2));

    let modal_area = centered_rect_fixed(modal_width, modal_height, area);

    // Clear the background
    f.render_widget(Clear, modal_area);

    // Build title with group indicator
    let group_indicator = format!(" ({}/{}) ", state.group_index + 1, state.total_groups);
    let title = if let Some(ref context) = state.data.context_label {
        format!(" Set {} - {}{}", state.data.tag_name, context, group_indicator)
    } else {
        format!(" Squash \"{}\" variants{}", state.data.tag_name, group_indicator)
    };

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner_area = block.inner(modal_area);
    f.render_widget(block, modal_area);

    // Split inner area: input field at TOP, variants list, controls hint
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Canonical input (TOP)
            Constraint::Min(4),    // Variants list
            Constraint::Length(2), // Controls hint
        ])
        .split(inner_area);

    // Render canonical input field AT TOP
    render_canonical_input(f, chunks[0], state);

    // Render variants list BELOW
    render_variants_list(f, chunks[1], state);

    // Render controls hint
    render_controls_hint(f, chunks[2], state);

    // Render modal overlay if present
    if state.has_modal() {
        render_modal_overlay(f, modal_area, state);
    }
}

/// Render the canonical value input field.
fn render_canonical_input(f: &mut Frame, area: Rect, state: &TagCanonicalityState) {
    let is_focused = state.cursor == -1;
    let input_style = if is_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
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
                Span::styled(c.to_string(), Style::default().fg(Color::Black).bg(Color::White)),
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

/// Render the variants list with checkboxes.
fn render_variants_list(f: &mut Frame, area: Rect, state: &TagCanonicalityState) {
    let items: Vec<ListItem> = state
        .data
        .variants
        .iter()
        .enumerate()
        .map(|(i, variant)| {
            let selected = state.selected.contains(&i);
            let is_cursor = state.cursor >= 0 && i == state.cursor as usize;

            let checkbox = if selected { "[x]" } else { "[ ]" };
            let text = format!("{} {} ({})", checkbox, variant.value, variant.count);

            let style = if is_cursor {
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

    let list_focused = state.cursor >= 0;
    let list_style = if list_focused {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let list = List::new(items).block(
        Block::default()
            .title(" Variants (Space to toggle) ")
            .borders(Borders::ALL)
            .border_style(list_style),
    );

    f.render_widget(list, area);
}

/// Render the controls hint at the bottom.
fn render_controls_hint(f: &mut Frame, area: Rect, state: &TagCanonicalityState) {
    let hints = if state.cursor == -1 {
        // On text field
        "[↑↓] navigate  [Enter] confirm  [Tab] next  [Ctrl+R] review  [Esc] cancel"
    } else {
        // On variant list
        "[Space] toggle  [↑↓] navigate  [Enter] confirm  [Tab] next  [Ctrl+R] review  [Esc] cancel"
    };

    let hint = Paragraph::new(hints)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);

    f.render_widget(hint, area);
}

/// Render the confirmation modal overlay.
///
/// Currently no modals are used - navigation is non-committal.
fn render_modal_overlay(_f: &mut Frame, _parent_area: Rect, state: &TagCanonicalityState) {
    match state.modal {
        TagCanonicalityModal::None => {}
        // Future modal types would be handled here
    }
}

