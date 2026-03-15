//! Rendering for the Release Review View.
//!
//! Full-screen StandardList with multi-select: each row shows checkbox +
//! release title + coverage. Wizard popup (z) shows release overview,
//! pane (Z) shows tracklist detail.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::{ReleaseReviewEntry, ReleaseReviewState};
use crate::helpers::truncate_right;
use crate::widgets::control_colors;
use crate::widgets::standard_list::render_standard_list;

pub(crate) fn render(f: &mut Frame, area: Rect, state: &mut ReleaseReviewState) {
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_title_bar(f, outer[0], state);
    render_controls(f, outer[2]);

    let ReleaseReviewState {
        ref entries,
        ref mut list,
        ..
    } = *state;

    render_standard_list(
        list,
        f,
        outer[1],
        entries,
        |idx, is_cursor, is_selected, width| render_row(entries, idx, is_cursor, is_selected, width),
        "Releases",
        true,
    );
}

fn render_title_bar(f: &mut Frame, area: Rect, state: &ReleaseReviewState) {
    let count = state.entries.len();
    let selected_count = state.list.selected.len();

    let mut spans = vec![
        Span::styled(
            " Release Review ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({} releases)", count),
            Style::default().fg(Color::DarkGray),
        ),
    ];

    if selected_count > 0 {
        spans.push(Span::styled(
            format!("  [{} selected]", selected_count),
            Style::default().fg(Color::Cyan),
        ));
    }

    f.render_widget(Paragraph::new(vec![Line::from(spans)]), area);
}

fn render_controls(f: &mut Frame, area: Rect) {
    let spans = vec![
        control_colors::text(" "),
        control_colors::nav("^v"),
        control_colors::text(" nav  "),
        control_colors::nav("Space"),
        control_colors::text(" toggle  "),
        control_colors::confirm("Enter"),
        control_colors::text(" approve  "),
        control_colors::nav("z/Z"),
        control_colors::text(" info  "),
        control_colors::cancel("Esc"),
        control_colors::text(" close"),
    ];
    f.render_widget(Paragraph::new(vec![Line::from(spans)]), area);
}

fn render_row(
    entries: &[ReleaseReviewEntry],
    idx: usize,
    is_cursor: bool,
    is_selected: bool,
    width: u16,
) -> Line<'static> {
    let Some(entry) = entries.get(idx) else {
        return Line::raw("");
    };

    let release = &entry.release;
    let title_style = if is_cursor {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let coverage = if release.track_count > 0 {
        release.matched_count as f64 / release.track_count as f64
    } else {
        0.0
    };
    let cov_color = if coverage >= 1.0 {
        Color::Green
    } else if coverage >= 0.7 {
        Color::Yellow
    } else {
        Color::Red
    };

    let checkbox = if is_selected { "[x] " } else { "[ ] " };
    let marker = if is_cursor { "\u{25b8} " } else { "  " };
    let prefix_len = 4 + 2; // checkbox + marker
    let suffix = format!(" {}/{}", release.matched_count, release.track_count);
    let title_max = (width as usize).saturating_sub(prefix_len + suffix.len());

    Line::from(vec![
        Span::styled(
            checkbox,
            if is_selected {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            },
        ),
        Span::styled(marker.to_string(), title_style),
        Span::styled(truncate_right(&release.title, title_max), title_style),
        Span::styled(suffix, Style::default().fg(cov_color)),
    ])
}
