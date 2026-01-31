//! Rendering for the Debug view.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

use super::{DebugViewState, SelectedOperation};

/// Render the debug view.
pub fn render_debug_view(f: &mut Frame, area: Rect, state: &DebugViewState) {
    // Layout: unified titlebar + content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()),
            Constraint::Min(0),
        ])
        .split(area);

    // Render unified titlebar
    let titlebar = UnifiedTitleBar::new(LateralView::Debug);
    titlebar.render(f, chunks[0]);

    // Content area
    render_content(f, chunks[1], state);
}

fn render_content(f: &mut Frame, area: Rect, state: &DebugViewState) {
    let content_block = Block::default()
        .borders(Borders::ALL)
        .title(" Debug & Maintenance ");

    let inner = content_block.inner(area);
    f.render_widget(content_block, area);

    if inner.height < 4 || inner.width < 20 {
        return;
    }

    // Split content into operations section and stats section
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(10),   // Operations section
            Constraint::Length(5), // Stats section
        ])
        .split(inner);

    render_operations_section(f, sections[0], state);
    render_stats_section(f, sections[1], state);
}

fn render_operations_section(f: &mut Frame, area: Rect, state: &DebugViewState) {
    let is_selected = state.selected == SelectedOperation::RebuildFingerprints;
    let border_style = if is_selected {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            " Rebuild All Fingerprints ",
            if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    // Description
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  Regenerates acoustic fingerprints for all tracks from the complete",
        Style::default().fg(Color::White),
    )));
    lines.push(Line::from(Span::styled(
        "  audio file (no duration limit).",
        Style::default().fg(Color::White),
    )));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "  This operation will:",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("*", Style::default().fg(Color::Yellow)),
        Span::styled(" Process all tracks in the corpus", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("*", Style::default().fg(Color::Yellow)),
        Span::styled(" Detect corruption in audio beyond the first 2 minutes", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("*", Style::default().fg(Color::Yellow)),
        Span::styled(" Update duplicate detection with more accurate fingerprints", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::raw("    "),
        Span::styled("*", Style::default().fg(Color::Yellow)),
        Span::styled(" Emit CorruptFile signals for tracks that fail to decode", Style::default().fg(Color::DarkGray)),
    ]));

    lines.push(Line::from(""));

    if is_selected && !state.witch_busy {
        lines.push(Line::from(Span::styled(
            "  [Enter] Execute",
            Style::default().fg(Color::Green),
        )));
    } else if state.witch_busy {
        lines.push(Line::from(Span::styled(
            "  Operation in progress...",
            Style::default().fg(Color::Yellow),
        )));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_stats_section(f: &mut Frame, area: Rect, state: &DebugViewState) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(Span::styled(" Corpus Stats ", Style::default().fg(Color::DarkGray)));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    lines.push(Line::from(vec![
        Span::raw("  Tracks: "),
        Span::styled(
            format!("{}", state.track_count),
            Style::default().fg(Color::Cyan),
        ),
    ]));

    lines.push(Line::from(vec![
        Span::raw("  Fingerprinted: "),
        Span::styled(
            format!("{}", state.fingerprinted_count),
            Style::default().fg(Color::Green),
        ),
        Span::raw(" / "),
        Span::styled(
            format!("{}", state.track_count),
            Style::default().fg(Color::Cyan),
        ),
    ]));

    let missing = state.track_count - state.fingerprinted_count;
    if missing > 0 {
        lines.push(Line::from(vec![
            Span::raw("  Missing fingerprints: "),
            Span::styled(
                format!("{}", missing),
                Style::default().fg(Color::Red),
            ),
        ]));
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}
