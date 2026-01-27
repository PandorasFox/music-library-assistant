//! Rendering for the Format Standardization view.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

use super::{FormatStdState, SelectedAction};

/// Render the format standardization view.
pub fn render(f: &mut Frame, area: Rect, state: &FormatStdState) {
    // Layout: unified titlebar + content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()),
            Constraint::Min(0),
        ])
        .split(area);

    // Render unified titlebar
    let titlebar = UnifiedTitleBar::new(LateralView::FormatStandardization);
    titlebar.render(f, chunks[0]);

    // Content area
    render_content(f, chunks[1], state);
}

fn render_content(f: &mut Frame, area: Rect, state: &FormatStdState) {
    let content_block = Block::default()
        .borders(Borders::ALL)
        .title(" Format Standardization ");

    let inner = content_block.inner(area);
    f.render_widget(content_block, area);

    if inner.height < 4 || inner.width < 20 {
        return;
    }

    // Split content into lossy section and lossless section
    let sections = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(6),    // Lossy section
            Constraint::Length(1), // Spacer
            Constraint::Min(6),    // Lossless section
        ])
        .split(inner);

    render_lossy_section(f, sections[0], state);
    render_lossless_section(f, sections[2], state);
}

fn render_lossy_section(f: &mut Frame, area: Rect, state: &FormatStdState) {
    let is_selected = state.selected == SelectedAction::ConvertLossy;
    let border_style = if is_selected {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let total = state.lossy_count();
    let title = format!(" Lossy -> Opus ({} kbps) ", state.opus_bitrate_kbps);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            title,
            if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  No lossy files to convert",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        // File type breakdown
        for (file_type, count) in state.lossy_breakdown() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:>5}", file_type.to_uppercase()),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(": "),
                Span::styled(
                    format!("{}", count),
                    Style::default().fg(Color::White),
                ),
                Span::styled(" files", Style::default().fg(Color::DarkGray)),
            ]));
        }

        lines.push(Line::from(""));

        // Total and action hint
        lines.push(Line::from(vec![
            Span::raw("  Total: "),
            Span::styled(
                format!("{}", total),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" files", Style::default().fg(Color::DarkGray)),
        ]));

        if is_selected {
            lines.push(Line::from(vec![
                Span::styled(
                    "  [Enter] Convert all to Opus",
                    Style::default().fg(Color::Green),
                ),
                Span::raw("  "),
                Span::styled(
                    "[</>] Adjust bitrate",
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_lossless_section(f: &mut Frame, area: Rect, state: &FormatStdState) {
    let is_selected = state.selected == SelectedAction::ConvertLossless;
    let border_style = if is_selected {
        Style::default().fg(Color::Cyan)
    } else {
        Style::default().fg(Color::DarkGray)
    };

    let total = state.lossless_count();

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border_style)
        .title(Span::styled(
            " Lossless -> FLAC ",
            if is_selected {
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let mut lines = Vec::new();

    if total == 0 {
        lines.push(Line::from(Span::styled(
            "  No lossless files to convert",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        // File type breakdown
        for (file_type, count) in state.lossless_breakdown() {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(
                    format!("{:>5}", file_type.to_uppercase()),
                    Style::default().fg(Color::Yellow),
                ),
                Span::raw(": "),
                Span::styled(
                    format!("{}", count),
                    Style::default().fg(Color::White),
                ),
                Span::styled(" files", Style::default().fg(Color::DarkGray)),
            ]));
        }

        lines.push(Line::from(""));

        lines.push(Line::from(vec![
            Span::raw("  Total: "),
            Span::styled(
                format!("{}", total),
                Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" files", Style::default().fg(Color::DarkGray)),
        ]));

        if is_selected {
            lines.push(Line::from(Span::styled(
                "  [Enter] Convert all to FLAC",
                Style::default().fg(Color::Green),
            )));
        }
    }

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}
