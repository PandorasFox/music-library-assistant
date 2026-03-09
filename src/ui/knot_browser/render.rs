//! Rendering for the Knot Browser.
//!
//! Two-pane layout with header bar:
//! - Header: knot identity, stats, position indicator
//! - Left (50%): competing releases with selected/rejected icons
//! - Right (50%): detail for selected release or knot overview
//! - Bottom: controls hint with sort toggle

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::types::*;
use super::{FocusedPane, KnotBrowserState};
use crate::ui::widgets::control_colors;
use crate::ui::widgets::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

pub fn render(f: &mut Frame, area: Rect, state: &mut KnotBrowserState) {
    if state.knots.is_empty() {
        let msg = Paragraph::new("No knots to display.")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, area);
        return;
    }

    // Outer: header (1) + content (min) + controls (1)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(f, outer[0], state);
    render_controls(f, outer[2], state);

    // Content: releases (50%) + detail (50%)
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(outer[1]);

    render_releases_pane(f, panes[0], state);
    render_detail_pane(f, panes[1], state);
}

fn render_header(f: &mut Frame, area: Rect, state: &KnotBrowserState) {
    let knot = match state.current_knot() {
        Some(k) => k,
        None => return,
    };

    let line = Line::from(vec![
        Span::styled(
            format!(
                " Knot {}/{} ",
                state.knot_index + 1,
                state.knots.len()
            ),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("— ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", knot.tier),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled(
            format!(
                "  {} releases / {} inodes  r={:.1}  ({})",
                knot.proposal_count, knot.inode_count, knot.ratio, knot.classification,
            ),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(vec![line]), area);
}

fn render_controls(f: &mut Frame, area: Rect, state: &KnotBrowserState) {
    let line = Line::from(vec![
        control_colors::text(" "),
        control_colors::nav("Tab/S-Tab"),
        control_colors::text(" knot  "),
        control_colors::nav("^v"),
        control_colors::text(" nav  "),
        control_colors::nav("Shift+Arrow"),
        control_colors::text(" pane  "),
        control_colors::toggle("s"),
        control_colors::text(&format!(" sort ({})", state.sort_mode.label())),
        control_colors::text("  "),
        control_colors::cancel("Esc"),
        control_colors::text(" close"),
    ]);
    f.render_widget(Paragraph::new(vec![line]), area);
}

// ============================================================================
// Releases Pane
// ============================================================================

fn render_releases_pane(f: &mut Frame, area: Rect, state: &mut KnotBrowserState) {
    let focused = matches!(state.focused_pane, FocusedPane::Releases);
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(
            format!(" Releases ({}) ", state.sort_mode.label()),
            Style::default().fg(Color::White),
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let proposal_count = match state.current_knot() {
        Some(k) => k.proposals.len(),
        None => return,
    };

    let visible_height = inner.height as usize;
    if visible_height == 0 {
        return;
    }

    // Adjust scroll to keep cursor visible
    if state.release_cursor < state.release_scroll {
        state.release_scroll = state.release_cursor;
    }
    if state.release_cursor >= state.release_scroll + visible_height {
        state.release_scroll = state.release_cursor + 1 - visible_height;
    }

    state.click_targets.clear();
    state.click_targets.set_list_area(inner);

    let max_title_width = inner.width.saturating_sub(18) as usize; // icon(2) + score(7) + inodes(5) + padding(4)

    let knot = &state.knots[state.knot_index];
    for (vis_idx, prop_idx) in (state.release_scroll..)
        .take(visible_height)
        .enumerate()
    {
        if prop_idx >= proposal_count {
            break;
        }
        let proposal = &knot.proposals[prop_idx];
        let is_cursor = prop_idx == state.release_cursor && focused;

        let icon = if proposal.selected { "✓ " } else { "✗ " };
        let icon_color = if proposal.selected {
            Color::Green
        } else {
            Color::Red
        };

        let title = truncate_for_width(&proposal.release_title, max_title_width);

        let base_style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };

        let line = Line::from(vec![
            Span::styled(icon.to_string(), Style::default().fg(icon_color)),
            Span::styled(title, base_style),
            Span::styled(
                format!("  {:.1}", proposal.total_score),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("  {}i", proposal.covered_inode_count),
                Style::default().fg(Color::DarkGray),
            ),
        ]);

        let y = inner.y + vis_idx as u16;
        f.render_widget(Paragraph::new(vec![line]), Rect::new(inner.x, y, inner.width, 1));

        state.click_targets.add_row(prop_idx.to_string(), y);
    }
}

// ============================================================================
// Detail Pane
// ============================================================================

fn render_detail_pane(f: &mut Frame, area: Rect, state: &KnotBrowserState) {
    let focused = matches!(state.focused_pane, FocusedPane::Detail);
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .title(Span::styled(" Detail ", Style::default().fg(Color::White)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let lines = if let Some(proposal) = state.selected_proposal() {
        render_proposal_detail(proposal)
    } else if let Some(knot) = state.current_knot() {
        render_knot_overview(knot)
    } else {
        vec![]
    };

    let skip = state.detail_scroll.min(lines.len().saturating_sub(1));
    let visible: Vec<Line> = lines.into_iter().skip(skip).collect();

    let paragraph = Paragraph::new(visible).wrap(Wrap { trim: false });
    f.render_widget(paragraph, inner);
}

fn render_knot_overview(knot: &KnotEntry) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Knot Overview",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        render_detail_field("Tier", &knot.tier),
        render_detail_field("Classification", &knot.classification),
        render_detail_field("Ratio", &format!("{:.1}", knot.ratio)),
        render_detail_field("Proposals", &knot.proposal_count.to_string()),
        render_detail_field("Contested inodes", &knot.inode_count.to_string()),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Contested Files:",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    for ci in &knot.contested_inodes {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {}x ", ci.claiming_release_count),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(ci.path.clone(), Style::default().fg(Color::White)),
        ]));
    }

    lines
}

fn render_proposal_detail(proposal: &KnotProposal) -> Vec<Line<'static>> {
    let status_text = if proposal.selected {
        "SELECTED"
    } else {
        "REJECTED"
    };
    let status_color = if proposal.selected {
        Color::Green
    } else {
        Color::Red
    };

    let mut lines = vec![
        Line::from(vec![
            Span::styled(
                proposal.release_title.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!("[{}]", status_text),
                Style::default().fg(status_color),
            ),
        ]),
        Line::from(Span::raw("")),
        render_detail_field("Artist", &proposal.release_artist),
        render_detail_field("MBID", &proposal.release_id),
        render_detail_field("Total tracks", &proposal.total_tracks.to_string()),
        render_detail_field("Score", &format!("{:.2}", proposal.total_score)),
        render_detail_field("Knot inodes", &proposal.covered_inode_count.to_string()),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Track Assignments:",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
    ];

    for track in &proposal.tracks {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>2}.{:>2} ", track.medium_pos, track.track_pos),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                track.track_title.clone(),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("  {:.2}", track.score),
                Style::default().fg(Color::Yellow),
            ),
        ]));
        lines.push(Line::from(Span::styled(
            format!("    {}", track.path),
            Style::default().fg(Color::DarkGray),
        )));

        // Score breakdown bars
        let scores = [
            ("  AcoustID:     ", track.score_breakdown.acoustid_confidence),
            ("  Duration:     ", track.score_breakdown.duration_match),
            ("  Title:        ", track.score_breakdown.title_match),
            ("  Artist:       ", track.score_breakdown.artist_match),
            ("  Album:        ", track.score_breakdown.album_match),
            ("  Track number: ", track.score_breakdown.track_number_match),
        ];
        for (label, value) in &scores {
            lines.push(render_score_bar(label, *value));
        }
        lines.push(Line::from(Span::raw("")));
    }

    lines
}

// ============================================================================
// Helpers
// ============================================================================

fn render_detail_field(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("  {}: ", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}

fn render_score_bar(label: &str, value: f64) -> Line<'static> {
    const BAR_WIDTH: usize = 20;
    let filled = ((value * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    let bar = format!("{}{}", "\u{2588}".repeat(filled), " ".repeat(empty));

    Line::from(vec![
        Span::styled(format!("  {}", label), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(" {:.2}  ", value),
            Style::default().fg(Color::White),
        ),
        Span::styled(bar, Style::default().fg(Color::Yellow)),
    ])
}

fn truncate_for_width(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars <= 3 {
        s.chars().take(max_chars).collect()
    } else {
        let mut result: String = s.chars().take(max_chars - 3).collect();
        result.push_str("...");
        result
    }
}
