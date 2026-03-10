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
use crate::ui::widgets::{ConfirmationButton, ConfirmationModal};

use super::types::{CompoundSplitStateV2, FocusPaneV2};

/// Render the three-pane compound split resolution interface.
pub fn render(f: &mut Frame, area: Rect, state: &mut CompoundSplitStateV2) {
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

    // Confirmation overlays
    if state.confirming_canonicalize {
        render_canonicalize_confirm(f, area, state);
    }
    if state.confirming_bulk_stage {
        render_bulk_stage_confirm(f, area, state);
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
fn render_three_panes(f: &mut Frame, area: Rect, state: &mut CompoundSplitStateV2) {
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
fn render_parts_pane(f: &mut Frame, area: Rect, state: &mut CompoundSplitStateV2) {
    let is_focused = state.focus_pane == FocusPaneV2::Parts && !state.is_editing();

    let title = format!(" SPLIT PARTS ({}) ", state.edited_parts.len());

    let block = crate::ui::helpers::focused_block(&title, is_focused);

    let inner = render_pane(f, area, block);

    state.parts_pane_rect = Some(area);
    let visible_height = inner.height as usize;
    let max_width = inner.width.saturating_sub(1) as usize;
    let scroll = crate::ui::helpers::clamp_scroll(state.part_cursor, state.part_scroll, visible_height);
    state.part_click_targets.populate(inner, scroll, state.edited_parts.len());

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

            let mut spans = vec![Span::styled(
                format!("{} ", indicator),
                Style::default().fg(indicator_color),
            )];

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
fn render_files_pane(f: &mut Frame, area: Rect, state: &mut CompoundSplitStateV2) {
    let is_focused = state.focus_pane == FocusPaneV2::Files && !state.is_editing();

    let selected_count = state.selected_files.len();
    let total_count = state.data.files.len();
    let title = format!(" TRACKS ({}/{}) ", selected_count, total_count);

    let block = crate::ui::helpers::focused_block(&title, is_focused);

    let inner = render_pane(f, area, block);

    state.files_pane_rect = Some(area);
    let visible_height = inner.height as usize;
    let max_width = inner.width.saturating_sub(1) as usize;
    let scroll = crate::ui::helpers::clamp_scroll(state.file_cursor, state.file_scroll, visible_height);
    state.file_click_targets.populate(inner, scroll, state.data.files.len());

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

    let max_width = inner.width.saturating_sub(1) as usize;
    let visible_height = inner.height as usize;

    let mut items: Vec<ListItem> = match file {
        Some(f) => {
            f.tag_values
                .iter()
                .take(visible_height)
                .map(|(name, value)| {
                    let text = format!("{}: {}", name, value);
                    let display = truncate_right(&text, max_width);

                    // Highlight the tag being split
                    let style = if name.to_uppercase()
                        == state.data.compound.tag_name.to_uppercase()
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

    // Show pending tag editor edits for this file, if any
    if let Some(f) = file.as_ref() {
        if let Some(ref edits_map) = state.pending_tag_edits {
            if let Some(edits) = edits_map.get(&f.inode) {
                let remaining = visible_height.saturating_sub(items.len());
                for line in crate::ui::helpers::render_pending_edit_lines(edits, max_width, remaining) {
                    items.push(ListItem::new(line));
                }
            }
        }
    }

    let list = List::new(items);
    f.render_widget(list, inner);
}

/// Render the edit input field (only shown when editing).
fn render_edit_input(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let block = Block::default()
        .title(" Edit part ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));

    let inner = render_pane(f, area, block);

    // Render the text input using UTF-8 safe cursor splits
    let (before, cursor_char, after_cursor) = state.part_input.cursor_splits();

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
    use crate::ui::widgets::control_colors as cc;

    let mut hints = Vec::new();

    if state.is_editing() {
        // Editing mode controls
        hints.extend([
            cc::confirm("[Enter]"),
            cc::text(" Save  "),
            cc::cancel("[Esc]"),
            cc::text(" Cancel"),
        ]);
    } else {
        // Normal mode controls
        if state.focus_pane == FocusPaneV2::Files {
            hints.extend([cc::toggle("[Space]"), cc::text(" toggle  ")]);
        }

        if !state.is_safe_mode && state.focus_pane == FocusPaneV2::Parts {
            hints.extend([cc::edit("[E]"), cc::text(" edit  ")]);
        }

        hints.extend([
            cc::confirm("[Enter]"),
            cc::text(" split  "),
            cc::action("[^F]"),
            cc::text(" keep  "),
            cc::edit("[T]"),
            cc::text(" edit tags  "),
        ]);

        if state.is_safe_mode {
            hints.extend([cc::confirm("[^A]"), cc::text(" all  ")]);
        }

        hints.extend([
            cc::review("[^R]"),
            cc::text(" review  "),
            cc::cancel("[Esc]"),
            cc::text(" cancel"),
        ]);
    }

    let para = Paragraph::new(Line::from(hints)).alignment(Alignment::Center);
    f.render_widget(para, area);
}

/// Render the canonicalize confirmation overlay.
fn render_canonicalize_confirm(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let value = &state.data.compound.compound_value;
    let tag_name = &state.data.compound.tag_name;

    ConfirmationModal::new(" Confirm Canonical Value ")
        .border_color(Color::Cyan)
        .fixed_size(60, 9)
        .message(vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("Confirm \""),
                Span::styled(value.as_str(), Style::default().fg(Color::Yellow)),
                Span::raw(format!("\" as a standalone {} value?", tag_name)),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "This will whitelist it — compound detection will skip this value.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .buttons(vec![
            ConfirmationButton::new("[Enter] Confirm", Color::Green).selected(true),
            ConfirmationButton::new("[Esc] Cancel", Color::White),
        ])
        .render(f, area);
}

/// Render the bulk stage-all confirmation overlay.
fn render_bulk_stage_confirm(f: &mut Frame, area: Rect, state: &CompoundSplitStateV2) {
    let count = state.total_groups;

    ConfirmationModal::new(" Confirm Bulk Stage ")
        .border_color(Color::Green)
        .fixed_size(60, 9)
        .message(vec![
            Line::from(""),
            Line::from(vec![
                Span::raw("Stage all "),
                Span::styled(format!("{}", count), Style::default().fg(Color::Yellow)),
                Span::raw(" safe compound splits?"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "All splits will be staged for review before commit.",
                Style::default().fg(Color::DarkGray),
            )),
        ])
        .buttons(vec![
            ConfirmationButton::new("[Enter] Stage All", Color::Green).selected(true),
            ConfirmationButton::new("[Esc] Cancel", Color::White),
        ])
        .render(f, area);
}
