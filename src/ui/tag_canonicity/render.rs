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

use super::types::{TagCanonicalityModal, TagCanonicalityState};

/// Render the tag canonicity resolution modal.
pub fn render(f: &mut Frame, area: Rect, state: &TagCanonicalityState) {
    // Calculate modal size - centered, 60x20 or smaller
    let modal_width = 60.min(area.width.saturating_sub(4));
    let modal_height = 20.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width.saturating_sub(modal_width)) / 2,
        y: (area.height.saturating_sub(modal_height)) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the background
    f.render_widget(Clear, modal_area);

    // Build title
    let title = if let Some(ref context) = state.data.context_label {
        format!(" Set {} - {} ", state.data.tag_name, context)
    } else {
        format!(" Squash \"{}\" variants ", state.data.tag_name)
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
fn render_modal_overlay(f: &mut Frame, _parent_area: Rect, state: &TagCanonicalityState) {
    match state.modal {
        TagCanonicalityModal::None => {}
        // Future modal types would be handled here
    }
}

// ============================================================================
// Review Screen Rendering
// ============================================================================

use super::types::TagCanonicityReviewState;

/// Render the tag canonicity review screen.
///
/// Shows summary of all pending decisions before final confirmation.
pub fn render_review(f: &mut Frame, area: Rect, state: &TagCanonicityReviewState) {
    // Calculate modal size - centered, 70x22 or smaller
    let modal_width = 70.min(area.width.saturating_sub(4));
    let modal_height = 22.min(area.height.saturating_sub(4));

    let modal_area = Rect {
        x: (area.width.saturating_sub(modal_width)) / 2,
        y: (area.height.saturating_sub(modal_height)) / 2,
        width: modal_width,
        height: modal_height,
    };

    // Clear the background
    f.render_widget(Clear, modal_area);

    // Outer block with title
    let title = format!(
        " Review: {} decision{}, {} mutation{} ",
        state.decisions.len(),
        if state.decisions.len() == 1 { "" } else { "s" },
        state.total_mutations(),
        if state.total_mutations() == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = block.inner(modal_area);
    f.render_widget(block, modal_area);

    // Layout: list area + button area
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),      // Decision list (expandable)
            Constraint::Length(1),   // Spacer
            Constraint::Length(1),   // Buttons
            Constraint::Length(1),   // Hints
        ])
        .split(inner);

    // Render decision list with scrolling
    render_review_list(f, chunks[0], state);

    // Render buttons
    render_review_buttons(f, chunks[2], state);

    // Render hints
    let hints = "[↑↓] scroll  [Y/Enter] confirm  [N/Esc] cancel";
    let hint = Paragraph::new(hints)
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center);
    f.render_widget(hint, chunks[3]);
}

/// Render the scrollable decision list.
fn render_review_list(f: &mut Frame, area: Rect, state: &TagCanonicityReviewState) {
    let visible_height = area.height as usize;

    // Calculate scroll offset to keep cursor visible
    let scroll_offset = if state.cursor >= visible_height {
        state.cursor - visible_height + 1
    } else {
        0
    };

    let items: Vec<ListItem> = state.decisions
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, decision)| {
            let is_selected = idx == state.cursor;

            let prefix = if is_selected { "▸ " } else { "  " };
            let text = format!(
                "{}{} ({} edit{}, {} track{})",
                prefix,
                decision.label,
                decision.mutation_count,
                if decision.mutation_count == 1 { "" } else { "s" },
                decision.track_count,
                if decision.track_count == 1 { "" } else { "s" },
            );

            let style = if is_selected {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            ListItem::new(text).style(style)
        })
        .collect();

    let list = List::new(items);
    f.render_widget(list, area);

    // Show scroll indicator if there's more content
    if state.decisions.len() > visible_height {
        let indicator = if scroll_offset > 0 && scroll_offset + visible_height < state.decisions.len() {
            "↑↓"
        } else if scroll_offset > 0 {
            "↑"
        } else {
            "↓"
        };

        let indicator_area = Rect {
            x: area.x + area.width.saturating_sub(3),
            y: area.y,
            width: 2,
            height: 1,
        };

        let indicator_widget = Paragraph::new(indicator)
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(indicator_widget, indicator_area);
    }
}

/// Render the Cancel/Confirm buttons.
fn render_review_buttons(f: &mut Frame, area: Rect, state: &TagCanonicityReviewState) {
    let cancel_style = if !state.confirm_focused {
        Style::default().fg(Color::Black).bg(Color::White)
    } else {
        Style::default().fg(Color::White)
    };

    let confirm_style = if state.confirm_focused {
        Style::default().fg(Color::Black).bg(Color::Green)
    } else {
        Style::default().fg(Color::Green)
    };

    let buttons = Line::from(vec![
        Span::raw("      "),
        Span::styled(" Cancel ", cancel_style),
        Span::raw("   "),
        Span::styled(" Confirm ", confirm_style),
        Span::raw("      "),
    ]);

    let buttons_para = Paragraph::new(buttons).alignment(Alignment::Center);
    f.render_widget(buttons_para, area);
}
