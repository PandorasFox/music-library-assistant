//! Rendering for the History lateral view.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::{
    ConflictDisposition, EditDetailState, HistoryPhase, HistoryViewState, JettisonAllState,
    JettisonSessionState,
};
use crate::helpers::truncate_right;
use crate::widgets::control_colors as cc;
use crate::widgets::standard_list::{render_standard_list, ListEntry};
use crate::widgets::{ConfirmationButton, ConfirmationModal};

pub(crate) fn render(f: &mut Frame, area: Rect, state: &mut HistoryViewState) {
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
            render_conflict_resolution(f, area, cr)
        }
    }
}

// ============================================================================
// Session List (Level 1 — StandardList)
// ============================================================================

fn render_session_list(f: &mut Frame, area: Rect, state: &mut HistoryViewState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Session list
            Constraint::Length(1), // Controls
        ])
        .split(area);

    if state.sessions.is_empty() {
        let block = Block::default()
            .title(" Edit History — Sessions ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray));
        let inner = block.inner(chunks[0]);
        f.render_widget(block, chunks[0]);
        let empty = Paragraph::new(Line::from(Span::styled(
            "No edit history recorded.",
            Style::default().fg(Color::DarkGray),
        )));
        f.render_widget(empty, inner);
    } else {
        let HistoryViewState { ref mut session_list, ref sessions, .. } = *state;
        render_standard_list(
            session_list,
            f,
            chunks[0],
            sessions,
            |idx, is_cursor, _is_selected, _width| {
                render_session_row(sessions, idx, is_cursor)
            },
            "Edit History — Sessions",
            true,
        );
    }

    // Controls hint
    let hints = Line::from(vec![
        cc::nav(" ↑↓"),
        cc::text(" navigate  "),
        cc::toggle("[Z]"),
        cc::text(" info  "),
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

fn render_session_row(
    sessions: &[super::SessionListEntry],
    idx: usize,
    is_cursor: bool,
) -> Line<'static> {
    let Some(entry) = sessions.get(idx) else {
        return Line::raw("");
    };
    let s = &entry.summary;

    let prefix = if is_cursor { "\u{25b8} " } else { "  " };
    let base_style = if is_cursor {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let timestamp = truncate_right(&s.earliest_at, 19);
    let label = if s.session_id.len() > 30 {
        truncate_right(&s.session_id, 30)
    } else {
        s.session_id.clone()
    };

    Line::from(vec![
        Span::styled(prefix, base_style),
        Span::styled(
            format!("{} │ {}", timestamp, label),
            base_style,
        ),
        Span::styled(
            format!(
                "  ({} edit{}, {} file{})",
                s.edit_count,
                if s.edit_count == 1 { "" } else { "s" },
                s.inode_count,
                if s.inode_count == 1 { "" } else { "s" },
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

// ============================================================================
// Session Detail (Level 2 — StandardList with multi-select)
// ============================================================================

fn render_session_detail(f: &mut Frame, area: Rect, state: &mut HistoryViewState) {
    let detail = match state.detail {
        Some(ref mut d) => d,
        None => return,
    };

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(5),    // Edit list
            Constraint::Length(1), // Controls
        ])
        .split(area);

    let selectable_count = detail
        .entries
        .iter()
        .filter(|e| e.is_selectable())
        .count();
    let selected_count = detail.detail_list.selected.len();
    let title = format!(
        "Session: {} — {} item{} ({} selected)",
        truncate_right(&detail.session_id, 30),
        selectable_count,
        if selectable_count == 1 { "" } else { "s" },
        selected_count,
    );

    let EditDetailState { ref mut detail_list, ref entries, .. } = *detail;
    render_standard_list(
        detail_list,
        f,
        chunks[0],
        entries,
        |idx, is_cursor, is_selected, width| {
            render_edit_row(entries, idx, is_cursor, is_selected, width)
        },
        &title,
        true,
    );

    // Controls hint
    let hints = Line::from(vec![
        cc::nav(" ↑↓"),
        cc::text(" navigate  "),
        cc::toggle("[Space]"),
        cc::text(" toggle  "),
        cc::toggle("[Z]"),
        cc::text(" diff  "),
        cc::confirm("[Enter]"),
        cc::text(" reverse selected  "),
        cc::cancel("[Esc]"),
        cc::text(" back"),
    ]);
    f.render_widget(Paragraph::new(hints), chunks[1]);
}

fn render_edit_row(
    entries: &[super::EditDetailEntry],
    idx: usize,
    is_cursor: bool,
    is_selected: bool,
    width: u16,
) -> Line<'static> {
    use super::EditDetailEntry;

    let Some(entry) = entries.get(idx) else {
        return Line::raw("");
    };

    match entry {
        EditDetailEntry::CommonEdit {
            field_name,
            old_value,
            new_value,
            paths,
            ..
        } => {
            let checkbox = if is_selected { "[x]" } else { "[ ]" };
            let prefix = if is_cursor { "\u{25b8}" } else { " " };
            let old = old_value.as_deref().unwrap_or("∅");
            let new = new_value.as_deref().unwrap_or("∅");
            let file_count = paths.len();

            let cursor_style = if is_cursor {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            Line::from(vec![
                Span::styled(format!("{} {} ", prefix, checkbox), cursor_style),
                Span::styled(field_name.clone(), Style::default().fg(Color::Cyan)),
                Span::styled(": ", Style::default().fg(Color::DarkGray)),
                Span::styled(old.to_string(), Style::default().fg(Color::Red)),
                Span::styled(" → ", Style::default().fg(Color::DarkGray)),
                Span::styled(new.to_string(), Style::default().fg(Color::Green)),
                Span::styled(
                    format!(
                        "  (across {} file{})",
                        file_count,
                        if file_count == 1 { "" } else { "s" },
                    ),
                    Style::default().fg(Color::DarkGray),
                ),
            ])
        }

        EditDetailEntry::PerFileSeparator => {
            let w = width as usize;
            let label = " Per-file edits ";
            let pad = w.saturating_sub(label.len());
            let left = pad / 2;
            let right = pad - left;
            let sep = format!(
                "{}{}{}",
                "\u{2500}".repeat(left),
                label,
                "\u{2500}".repeat(right),
            );
            Line::styled(sep, Style::default().fg(Color::DarkGray))
        }

        EditDetailEntry::FileHeader { path } => {
            let path_max = ((width as usize) / 2).clamp(30, 100);
            let path_display = if path.chars().count() > path_max {
                truncate_right(path, path_max)
            } else {
                path.clone()
            };
            Line::styled(
                format!("  {}", path_display),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )
        }

        EditDetailEntry::FileEdit {
            edit, path, ..
        } => {
            let checkbox = if is_selected { "[x]" } else { "[ ]" };
            let prefix = if is_cursor { "\u{25b8}" } else { " " };
            let old = edit.old_value.as_deref().unwrap_or("∅");
            let new = edit.new_value.as_deref().unwrap_or("∅");

            let cursor_style = if is_cursor {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if is_selected {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default().fg(Color::White)
            };

            // When under a FileHeader, indent and skip showing the path.
            // Detect: if there's a FileHeader above us, we're grouped.
            let has_header = idx > 0
                && matches!(
                    entries.get(idx - 1),
                    Some(EditDetailEntry::FileHeader { .. } | EditDetailEntry::FileEdit { .. })
                );

            if has_header {
                Line::from(vec![
                    Span::styled(format!("    {} {} ", prefix, checkbox), cursor_style),
                    Span::styled(edit.field_name.clone(), Style::default().fg(Color::Cyan)),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(old.to_string(), Style::default().fg(Color::Red)),
                    Span::styled(" → ", Style::default().fg(Color::DarkGray)),
                    Span::styled(new.to_string(), Style::default().fg(Color::Green)),
                ])
            } else {
                // Standalone file edit (no header above) — show path
                let path_max = ((width as usize) / 3).clamp(25, 80);
                let path_short = if path.chars().count() > path_max {
                    truncate_right(path, path_max)
                } else {
                    path.clone()
                };

                Line::from(vec![
                    Span::styled(format!("{} {} ", prefix, checkbox), cursor_style),
                    Span::styled(path_short, cursor_style),
                    Span::styled(" │ ", Style::default().fg(Color::DarkGray)),
                    Span::styled(edit.field_name.clone(), Style::default().fg(Color::Cyan)),
                    Span::styled(": ", Style::default().fg(Color::DarkGray)),
                    Span::styled(old.to_string(), Style::default().fg(Color::Red)),
                    Span::styled(" → ", Style::default().fg(Color::DarkGray)),
                    Span::styled(new.to_string(), Style::default().fg(Color::Green)),
                ])
            }
        }
    }
}

// ============================================================================
// Conflict Resolution (unchanged — bespoke for now)
// ============================================================================

fn render_conflict_resolution(
    f: &mut Frame,
    area: Rect,
    state: &mut super::ConflictResolutionState,
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
        state.click_targets.clear();
        let msg = Paragraph::new(Line::from(Span::styled(
            "No conflicts — all reversals are clean. Press Enter to confirm.",
            Style::default().fg(Color::Green),
        )));
        f.render_widget(msg, inner);
    } else {
        let visible_height = inner.height as usize;
        let scroll = crate::helpers::clamp_scroll(state.conflict_cursor, state.conflict_scroll, visible_height);

        state.click_targets.populate(inner, scroll, state.conflicts.len());

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
// Jettison Confirmation Modals (unchanged)
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
