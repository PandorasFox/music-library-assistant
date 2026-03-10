//! Rendering for the Knot Browser.
//!
//! Layout: header (1 line) + StandardList (proposals) + controls (1 line).
//! Proposal detail shown via wizard pane (Z key).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::types::*;
use super::KnotBrowserState;
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

    let proposals = state
        .knots
        .get(state.knot_index)
        .map(|k| k.proposals.as_slice())
        .unwrap_or(&[]);
    let sort_label = state.sort_mode.label();

    state.list.render(
        f,
        outer[1],
        proposals,
        |idx, is_cursor, _is_selected, _width| render_proposal_item(proposals, idx, is_cursor),
        &format!("Releases ({})", sort_label),
        true,
    );
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
        control_colors::toggle("Z"),
        control_colors::text(" detail  "),
        control_colors::toggle("s"),
        control_colors::text(&format!(" sort ({})", state.sort_mode.label())),
        control_colors::text("  "),
        control_colors::cancel("Esc"),
        control_colors::text(" close"),
    ]);
    f.render_widget(Paragraph::new(vec![line]), area);
}

fn render_proposal_item(
    proposals: &[KnotProposal],
    idx: usize,
    is_cursor: bool,
) -> Line<'static> {
    let Some(proposal) = proposals.get(idx) else {
        return Line::raw("");
    };

    let icon = if proposal.selected { "✓ " } else { "✗ " };
    let icon_color = if proposal.selected {
        Color::Green
    } else {
        Color::Red
    };

    let base_style = if is_cursor { CURSOR_STYLE } else { LIST_ITEM_STYLE };

    Line::from(vec![
        Span::styled(icon.to_string(), Style::default().fg(icon_color)),
        Span::styled(proposal.release_title.clone(), base_style),
        Span::styled(
            format!("  {:.1}", proposal.total_score),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("  {}i", proposal.covered_inode_count),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}

// ============================================================================
// Proposal detail lines (used by WizardItem impl)
// ============================================================================

pub(super) fn build_proposal_detail_lines(proposal: &KnotProposal) -> Vec<Line<'static>> {
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
            "Track Assignments:".to_string(),
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
