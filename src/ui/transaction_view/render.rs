//! Rendering for the Transaction tab view.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph};
use ratatui::Frame;

use super::{TransactionButtonFocus, TransactionViewState};
use crate::ui::transaction_review::DecisionSummary;
use crate::ui::widgets::{centered_rect_fixed, ConfirmationButton, render_button_row};

/// Render the transaction view.
pub fn render(
    f: &mut Frame,
    area: Rect,
    state: &TransactionViewState,
    decisions: &[DecisionSummary],
) {
    if decisions.is_empty() {
        render_empty(f, area);
    } else {
        render_split(f, area, state, decisions);
    }

    // Confirmation popup overlay for decision removal
    render_removal_popup(f, area, state, decisions);
}

/// Empty state — no decisions staged.
fn render_empty(f: &mut Frame, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),    // Empty message
            Constraint::Length(1), // Hints
        ])
        .split(area);

    let empty = Paragraph::new("No decisions staged")
        .style(Style::default().fg(Color::DarkGray))
        .alignment(Alignment::Center)
        .block(Block::default().borders(Borders::NONE));
    f.render_widget(empty, chunks[0]);

    render_hints(f, chunks[1], &[]);
}

/// Two-pane layout: decisions (top ~30%) + diffs (bottom ~50%) + controls.
fn render_split(
    f: &mut Frame,
    area: Rect,
    state: &TransactionViewState,
    decisions: &[DecisionSummary],
) {
    // Top pane gets ~30% of available space, bottom pane gets the rest.
    // Buttons (3 lines) and hints (1 line) are fixed at the bottom.
    let top_height = ((area.height as u32) * 30 / 100).clamp(3, 12) as u16;

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(top_height), // Decision list
            Constraint::Min(3),             // Diff/mutation detail
            Constraint::Length(3),           // Buttons
            Constraint::Length(1),           // Hints
        ])
        .split(area);

    // Top pane: decision list
    render_decision_list(f, chunks[0], state, decisions);

    // Bottom pane: diff entries for selected decision
    let cursor = state.cursor.min(decisions.len().saturating_sub(1));
    if let Some(decision) = decisions.get(cursor) {
        crate::ui::transaction_review::render_diff_entries(f, chunks[1], &decision.diff_entries);
    }

    // Buttons (only highlight when button row is focused)
    let bf = state.buttons_focused;
    render_button_row(f, chunks[2], &[
        ConfirmationButton::new("Discard", Color::Red)
            .selected(bf && state.button_focus == TransactionButtonFocus::Discard),
        ConfirmationButton::new("Confirm", Color::Green)
            .selected(bf && state.button_focus == TransactionButtonFocus::Confirm),
    ]);

    // Hints
    render_hints(f, chunks[3], decisions);
}

/// Render the scrollable decision list.
fn render_decision_list(
    f: &mut Frame,
    area: Rect,
    state: &TransactionViewState,
    decisions: &[DecisionSummary],
) {
    let visible_height = area.height as usize;
    let cursor = state.cursor.min(decisions.len().saturating_sub(1));

    let scroll_offset = if cursor >= state.scroll + visible_height {
        cursor - visible_height + 1
    } else if cursor < state.scroll {
        cursor
    } else {
        state.scroll
    };

    let items: Vec<ListItem> = decisions
        .iter()
        .enumerate()
        .skip(scroll_offset)
        .take(visible_height)
        .map(|(idx, decision)| {
            let is_selected = idx == cursor;
            let prefix = if is_selected { "> " } else { "  " };
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
}

/// Render hints bar with summary and controls.
fn render_hints(f: &mut Frame, area: Rect, decisions: &[DecisionSummary]) {
    use crate::ui::widgets::control_colors as cc;

    let total_mutations: usize = decisions.iter().map(|d| d.mutation_count).sum();
    let summary = format!(
        "{} decision{}, {} mutation{}",
        decisions.len(),
        if decisions.len() == 1 { "" } else { "s" },
        total_mutations,
        if total_mutations == 1 { "" } else { "s" },
    );

    let hints = if decisions.is_empty() {
        Line::from(vec![
            cc::text(&summary),
        ])
    } else {
        Line::from(vec![
            cc::text(&summary),
            cc::text("  "),
            cc::nav("[Shift+<>]"),
            cc::text(" select  "),
            cc::confirm("[Enter]"),
            cc::text(" activate  "),
            cc::cancel("[x]"),
            cc::text(" remove"),
        ])
    };
    let hint_widget = Paragraph::new(hints).alignment(Alignment::Center);
    f.render_widget(hint_widget, area);
}

/// Render removal confirmation popup overlay.
fn render_removal_popup(
    f: &mut Frame,
    area: Rect,
    state: &TransactionViewState,
    decisions: &[DecisionSummary],
) {
    let Some(ref key) = state.pending_removal else { return };

    let label = decisions.iter()
        .find(|d| d.key == *key)
        .map(|d| d.label.as_str())
        .unwrap_or("this decision");

    let popup_width = 50.min(area.width.saturating_sub(4));
    let popup_area = centered_rect_fixed(popup_width, 7, area);
    f.render_widget(Clear, popup_area);

    let block = Block::default()
        .title(" Remove Decision ")
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Red));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let popup_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(1),    // Message
            Constraint::Length(1), // Hint
        ])
        .split(inner);

    let msg = Paragraph::new(vec![
        Line::from(vec![
            Span::styled("Remove ", Style::default().fg(Color::White)),
            Span::styled(
                crate::ui::helpers::truncate_right(label, 30),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled("?", Style::default().fg(Color::White)),
        ]),
    ])
    .alignment(Alignment::Center);
    f.render_widget(msg, popup_chunks[0]);

    use crate::ui::widgets::control_colors as cc;
    let hint = Paragraph::new(Line::from(vec![
        cc::confirm("[Enter]"),
        cc::text(" remove  "),
        cc::cancel("[Esc]"),
        cc::text(" cancel"),
    ]))
    .alignment(Alignment::Center);
    f.render_widget(hint, popup_chunks[1]);
}
