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

use crate::ui::widgets::rich_text::{RichBlock, RichSpan};

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

pub(super) fn build_proposal_detail_blocks(proposal: &KnotProposal) -> Vec<RichBlock> {
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

    let dim = Style::default().fg(Color::DarkGray);
    let white = Style::default().fg(Color::White);

    let detail_field = |label: &str, value: &str| -> RichBlock {
        RichBlock::Paragraph(vec![
            RichSpan::new(format!("  {}: ", label), dim),
            RichSpan::new(value.to_string(), white),
        ])
    };

    let mut blocks = vec![
        RichBlock::Paragraph(vec![
            RichSpan::new(
                proposal.release_title.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            RichSpan::new(" ", Style::default()),
            RichSpan::new(format!("[{}]", status_text), Style::default().fg(status_color)),
        ]),
        RichBlock::Blank,
        detail_field("Artist", &proposal.release_artist),
        detail_field("MBID", &proposal.release_id),
        detail_field("Total tracks", &proposal.total_tracks.to_string()),
        detail_field("Score", &format!("{:.2}", proposal.total_score)),
        detail_field("Knot inodes", &proposal.covered_inode_count.to_string()),
        RichBlock::Blank,
        RichBlock::Heading("Track Assignments:".to_string()),
    ];

    for track in &proposal.tracks {
        blocks.push(RichBlock::Paragraph(vec![
            RichSpan::new(
                format!("  {:>2}.{:>2} ", track.medium_pos, track.track_pos),
                dim,
            ),
            RichSpan::new(track.track_title.clone(), white),
            RichSpan::new(
                format!("  {:.2}", track.score),
                Style::default().fg(Color::Yellow),
            ),
        ]));
        blocks.push(RichBlock::Paragraph(vec![RichSpan::new(
            format!("    {}", track.path),
            dim,
        )]));

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
            blocks.push(render_score_bar_block(label, *value));
        }
        blocks.push(RichBlock::Blank);
    }

    blocks
}

// ============================================================================
// Helpers
// ============================================================================

fn render_score_bar_block(label: &str, value: f64) -> RichBlock {
    const BAR_WIDTH: usize = 20;
    let filled = ((value * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    let bar = format!("{}{}", "\u{2588}".repeat(filled), " ".repeat(empty));

    RichBlock::Paragraph(vec![
        RichSpan::new(format!("  {}", label), Style::default().fg(Color::DarkGray)),
        RichSpan::new(
            format!(" {:.2}  ", value),
            Style::default().fg(Color::White),
        ),
        RichSpan::new(bar, Style::default().fg(Color::Yellow)),
    ])
}

