//! Rendering for the Transaction tab view.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use super::{TransactionButtonFocus, TransactionViewState};
use crate::ui::transaction_review::DecisionSummary;
use crate::ui::widgets::{ConfirmationButton, render_button_row};

/// Render the transaction view.
pub fn render(
    f: &mut Frame,
    area: Rect,
    state: &TransactionViewState,
    decisions: &[DecisionSummary],
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(3),     // Decision list
            Constraint::Length(3),  // Buttons row
            Constraint::Length(1),  // Hints
        ])
        .split(area);

    // Decision list
    if decisions.is_empty() {
        let empty = Paragraph::new("No decisions staged")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(empty, chunks[0]);
    } else {
        let visible_height = chunks[0].height as usize;
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
        f.render_widget(list, chunks[0]);
    }

    // Buttons row (matches TransactionReview modal)
    if !decisions.is_empty() {
        render_button_row(f, chunks[1], &[
            ConfirmationButton::new("Discard", Color::Red)
                .selected(state.button_focus == TransactionButtonFocus::Discard),
            ConfirmationButton::new("Confirm", Color::Green)
                .selected(state.button_focus == TransactionButtonFocus::Confirm),
        ]);
    }

    // Hints bar
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
            cc::text(" activate"),
        ])
    };
    let hint_widget = Paragraph::new(hints).alignment(Alignment::Center);
    f.render_widget(hint_widget, chunks[2]);
}
