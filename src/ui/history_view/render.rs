//! Rendering for the History lateral view.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Scrollbar, ScrollbarOrientation, ScrollbarState},
    Frame,
};

use super::{
    ConflictDisposition, HistoryPhase, HistoryViewState, JettisonAllState, JettisonSessionState,
};
use crate::ui::helpers::truncate_right;
use crate::ui::widgets::control_colors as cc;
use crate::ui::widgets::{ConfirmationButton, ConfirmationModal};

pub fn render(f: &mut Frame, area: Rect, state: &mut HistoryViewState) {
    match state.phase {
        HistoryPhase::SessionList
        | HistoryPhase::ConfirmJettisonSession(_)
        | HistoryPhase::ConfirmJettisonAll(_)
        | HistoryPhase::ConfirmJettisonAllFinal(_) => {
            render_session_list(f, area, state);
            // Overlays
            match &state.phase {
                HistoryPhase::ConfirmJettisonSession(js) => {
                    render_confirm_jettison_session(f, area, js)
                }
                HistoryPhase::ConfirmJettisonAll(ja) => render_confirm_jettison_all(f, area, ja),
                HistoryPhase::ConfirmJettisonAllFinal(ja) => {
                    render_confirm_jettison_all_final(f, area, ja)
                }
                _ => {}
            }
        }
        HistoryPhase::SessionDetail => render_session_detail(f, area, state),
        HistoryPhase::ConflictResolution(ref mut cr) => {
            render_conflict_resolution(f, area, &mut state.click_targets, cr)
        }
    }
}

// ============================================================================
// Session List
// ============================================================================

fn render_session_list(f: &mut Frame, area: Rect, state: &mut HistoryViewState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Session list
            Constraint::Length(1), // Controls
        ])
        .split(area);

    let block = Block::default()
        .title(" Edit History — Sessions ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(chunks[0]);
    f.render_widget(block, chunks[0]);

    if state.sessions.is_empty() {
        state.click_targets.clear();
        let empty = Paragraph::new(Line::from(Span::styled(
            "No edit history recorded.",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(empty, inner);
    } else {
        let visible_height = inner.height as usize;
        let scroll = compute_scroll(state.cursor, state.scroll, visible_height);

        // Populate click targets
        state.click_targets.clear();
        state.click_targets.set_list_area(inner);
        for (vis_idx, entry_idx) in (scroll..).take(visible_height).enumerate() {
            if entry_idx >= state.sessions.len() {
                break;
            }
            state
                .click_targets
                .add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
        }

        let mut lines = Vec::new();
        for (i, session) in state
            .sessions
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible_height)
        {
            let is_selected = i == state.cursor;

            let timestamp = truncate_right(&session.earliest_at, 19);
            let label = if session.session_id.len() > 30 {
                truncate_right(&session.session_id, 30)
            } else {
                session.session_id.clone()
            };

            let text = format!(
                " {} │ {} │ {} edit{}, {} file{}",
                timestamp,
                label,
                session.edit_count,
                if session.edit_count == 1 { "" } else { "s" },
                session.inode_count,
                if session.inode_count == 1 { "" } else { "s" },
            );

            let style = if is_selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            lines.push(Line::from(Span::styled(text, style)));
        }

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, inner);

        // Scrollbar
        if state.sessions.len() > visible_height {
            let mut scrollbar_state =
                ScrollbarState::new(state.sessions.len()).position(state.cursor);
            f.render_stateful_widget(
                Scrollbar::new(ScrollbarOrientation::VerticalRight),
                chunks[0],
                &mut scrollbar_state,
            );
        }
    }

    // Controls hint
    let hints = Line::from(vec![
        cc::nav(" ↑↓"),
        cc::text(" navigate  "),
        cc::confirm("[Enter]"),
        cc::text(" expand  "),
        cc::action("[d]"),
        cc::text(" jettison  "),
        cc::action("[D]"),
        cc::text(" jettison all  "),
        cc::cancel("[Esc]"),
        cc::text(" quit"),
    ]);
    f.render_widget(Paragraph::new(hints), chunks[1]);
}

// ============================================================================
// Session Detail
// ============================================================================

fn render_session_detail(f: &mut Frame, area: Rect, state: &mut HistoryViewState) {
    let detail = match state.detail {
        Some(ref d) => d,
        None => return,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Edit list
            Constraint::Length(1), // Controls
        ])
        .split(area);

    let selected_count = detail.selected.len();
    let title = format!(
        " Session: {} — {} edit{} ({} selected) ",
        truncate_right(&detail.session_id, 30),
        detail.edits.len(),
        if detail.edits.len() == 1 { "" } else { "s" },
        selected_count,
    );

    let block = Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(chunks[0]);
    f.render_widget(block, chunks[0]);

    let visible_height = inner.height as usize;
    let scroll = compute_scroll(detail.detail_cursor, detail.detail_scroll, visible_height);

    // Populate click targets
    state.click_targets.clear();
    state.click_targets.set_list_area(inner);
    for (vis_idx, entry_idx) in (scroll..).take(visible_height).enumerate() {
        if entry_idx >= detail.edits.len() {
            break;
        }
        state
            .click_targets
            .add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
    }

    let mut lines = Vec::new();
    for (i, edit) in detail
        .edits
        .iter()
        .enumerate()
        .skip(scroll)
        .take(visible_height)
    {
        let is_cursor = i == detail.detail_cursor;
        let is_selected = detail.selected.contains(&i);

        let checkbox = if is_selected { "[x]" } else { "[ ]" };

        let path = detail
            .inode_paths
            .get(&edit.inode)
            .map(|s| s.as_str())
            .unwrap_or("?");
        let path_short = if path.len() > 25 {
            truncate_right(path, 25)
        } else {
            path.to_string()
        };

        let old = edit.old_value.as_deref().unwrap_or("∅");
        let new = edit.new_value.as_deref().unwrap_or("∅");
        let time_short = truncate_right(&edit.edited_at, 19);

        let text = format!(
            " {} {} │ {} │ {} → {} │ {}",
            checkbox, path_short, edit.field_name, old, new, time_short,
        );

        let style = if is_cursor {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if is_selected {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default().fg(Color::White)
        };

        lines.push(Line::from(Span::styled(text, style)));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);

    // Scrollbar
    if detail.edits.len() > visible_height {
        let mut scrollbar_state =
            ScrollbarState::new(detail.edits.len()).position(detail.detail_cursor);
        f.render_stateful_widget(
            Scrollbar::new(ScrollbarOrientation::VerticalRight),
            chunks[0],
            &mut scrollbar_state,
        );
    }

    // Controls hint
    let hints = Line::from(vec![
        cc::nav(" ↑↓"),
        cc::text(" navigate  "),
        cc::toggle("[Space]"),
        cc::text(" toggle  "),
        cc::confirm("[Enter]"),
        cc::text(" reverse selected  "),
        cc::cancel("[Esc]"),
        cc::text(" back"),
    ]);
    f.render_widget(Paragraph::new(hints), chunks[1]);
}

// ============================================================================
// Conflict Resolution
// ============================================================================

fn render_conflict_resolution(
    f: &mut Frame,
    area: Rect,
    click_targets: &mut crate::ui::widgets::ListClickTargets,
    state: &super::ConflictResolutionState,
) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Summary header
            Constraint::Min(5),    // Conflict list
            Constraint::Length(1), // Controls
        ])
        .split(area);

    // Summary header
    let header_text = format!(
        " {} clean reversal{}, {} conflict{} ",
        state.clean_reversals.len(),
        if state.clean_reversals.len() == 1 {
            ""
        } else {
            "s"
        },
        state.conflicts.len(),
        if state.conflicts.len() == 1 { "" } else { "s" },
    );
    let header = Paragraph::new(Line::from(Span::styled(
        header_text,
        Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
    )))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::Yellow)),
    );
    f.render_widget(header, chunks[0]);

    // Conflict list
    let block = Block::default()
        .title(" Conflicts — decide per edit ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));
    let inner = block.inner(chunks[1]);
    f.render_widget(block, chunks[1]);

    if state.conflicts.is_empty() {
        click_targets.clear();
        let msg = Paragraph::new(Line::from(Span::styled(
            "No conflicts — all reversals are clean. Press Enter to confirm.",
            Style::default().fg(Color::Green),
        )));
        f.render_widget(msg, inner);
    } else {
        let visible_height = inner.height as usize;
        let scroll = compute_scroll(state.conflict_cursor, state.conflict_scroll, visible_height);

        // Populate click targets
        click_targets.clear();
        click_targets.set_list_area(inner);
        for (vis_idx, entry_idx) in (scroll..).take(visible_height).enumerate() {
            if entry_idx >= state.conflicts.len() {
                break;
            }
            click_targets.add_row(entry_idx.to_string(), inner.y + vis_idx as u16);
        }

        let mut lines = Vec::new();
        for (i, conflict) in state
            .conflicts
            .iter()
            .enumerate()
            .skip(scroll)
            .take(visible_height)
        {
            let is_cursor = i == state.conflict_cursor;

            let disposition_label = match conflict.disposition {
                ConflictDisposition::RevertAnyway => "REVERT",
                ConflictDisposition::Skip => "SKIP  ",
            };

            let old = conflict.edit.old_value.as_deref().unwrap_or("∅");
            let new = conflict.edit.new_value.as_deref().unwrap_or("∅");
            let current = conflict.current_value.as_deref().unwrap_or("∅");

            let text = format!(
                " [{}] #{} {} │ was {} → set {} → now {}",
                disposition_label, conflict.edit.id, conflict.edit.field_name, old, new, current,
            );

            let style = if is_cursor {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                match conflict.disposition {
                    ConflictDisposition::RevertAnyway => Style::default().fg(Color::Yellow),
                    ConflictDisposition::Skip => Style::default().fg(Color::DarkGray),
                }
            };

            lines.push(Line::from(Span::styled(text, style)));
        }

        let paragraph = Paragraph::new(lines);
        f.render_widget(paragraph, inner);
    }

    // Controls hint
    let hints = Line::from(vec![
        cc::nav(" ↑↓"),
        cc::text(" navigate  "),
        cc::toggle("[Space]"),
        cc::text(" revert/skip  "),
        cc::confirm("[Enter]"),
        cc::text(" confirm  "),
        cc::cancel("[Esc]"),
        cc::text(" cancel"),
    ]);
    f.render_widget(Paragraph::new(hints), chunks[2]);
}

// ============================================================================
// Jettison Confirmation Modals
// ============================================================================

fn render_confirm_jettison_session(f: &mut Frame, area: Rect, state: &JettisonSessionState) {
    let label = truncate_right(&state.session_id, 30);
    ConfirmationModal::new(" Jettison Session ")
        .border_color(Color::Yellow)
        .fixed_size(60, 9)
        .message(vec![
            Line::from(""),
            Line::from(Span::styled(
                format!("Jettison session '{}'?", label),
                Style::default().fg(Color::Yellow),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "{} edit{} will be exported to log and deleted.",
                    state.edit_count,
                    if state.edit_count == 1 { "" } else { "s" },
                ),
                Style::default().fg(Color::White),
            )),
        ])
        .buttons(vec![
            ConfirmationButton::new("Yes", Color::Green).selected(true),
            ConfirmationButton::new("No", Color::Yellow),
        ])
        .hint("Enter confirm  •  Esc cancel")
        .render(f, area);
}

fn render_confirm_jettison_all(f: &mut Frame, area: Rect, state: &JettisonAllState) {
    ConfirmationModal::new(" Jettison ALL Edit History ")
        .border_color(Color::Red)
        .fixed_size(60, 9)
        .message(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Jettison ALL edit history?",
                Style::default().fg(Color::Red),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "{} record{} across {} session{}",
                    state.total_records,
                    if state.total_records == 1 { "" } else { "s" },
                    state.session_count,
                    if state.session_count == 1 { "" } else { "s" },
                ),
                Style::default().fg(Color::White),
            )),
        ])
        .buttons(vec![
            ConfirmationButton::new("Yes, proceed", Color::Red).selected(true),
            ConfirmationButton::new("No", Color::Green),
        ])
        .hint("Enter proceed  •  Esc cancel")
        .render(f, area);
}

fn render_confirm_jettison_all_final(f: &mut Frame, area: Rect, state: &JettisonAllState) {
    ConfirmationModal::new(" ARE YOU SURE? ")
        .border_color(Color::Red)
        .fixed_size(64, 9)
        .message(vec![
            Line::from(""),
            Line::from(Span::styled(
                "Are you REALLY sure?",
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!(
                    "All {} record{} will be exported and permanently deleted.",
                    state.total_records,
                    if state.total_records == 1 { "" } else { "s" },
                ),
                Style::default().fg(Color::White),
            )),
        ])
        .buttons(vec![
            ConfirmationButton::new("Yes, jettison all", Color::Red).selected(true),
            ConfirmationButton::new("Cancel", Color::Green),
        ])
        .hint("Enter jettison  •  Esc cancel")
        .render(f, area);
}

// ============================================================================
// Helpers
// ============================================================================

fn compute_scroll(cursor: usize, current_scroll: usize, visible_height: usize) -> usize {
    if visible_height == 0 {
        return 0;
    }
    if cursor < current_scroll {
        cursor
    } else if cursor >= current_scroll + visible_height {
        cursor - visible_height + 1
    } else {
        current_scroll
    }
}
