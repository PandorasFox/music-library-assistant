//! Rendering for the AcoustID Browse View.
//!
//! Full-screen StandardList: each row shows path (truncated left) + confidence %
//! + recording title. Wizard popup shows confidence + MB recording info.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use super::AcoustidBrowseState;
use crate::helpers::truncate_left;
use crate::widgets::control_colors;
use crate::widgets::standard_list::render_standard_list;

pub(crate) fn render(f: &mut Frame, area: Rect, state: &mut AcoustidBrowseState) {
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

    let AcoustidBrowseState {
        ref items,
        ref mut list,
    } = *state;

    render_standard_list(
        list,
        f,
        outer[1],
        items,
        |idx, is_cursor, _is_selected, width| render_row(items, idx, is_cursor, width),
        "Matches",
        true,
    );
}

fn render_title_bar(f: &mut Frame, area: Rect, state: &AcoustidBrowseState) {
    let count = state.items.len();
    let spans = vec![
        Span::styled(
            " AcoustID Browse ",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({} entries)", count),
            Style::default().fg(Color::DarkGray),
        ),
    ];
    f.render_widget(Paragraph::new(vec![Line::from(spans)]), area);
}

fn render_controls(f: &mut Frame, area: Rect) {
    let spans = vec![
        control_colors::text(" "),
        control_colors::nav("^v"),
        control_colors::text(" nav  "),
        control_colors::confirm("Enter"),
        control_colors::text(" open MB  "),
        control_colors::nav("z"),
        control_colors::text(" info  "),
        control_colors::cancel("Esc"),
        control_colors::text(" close"),
    ];
    f.render_widget(Paragraph::new(vec![Line::from(spans)]), area);
}

fn render_row(
    items: &[super::AcoustidBrowseItem],
    idx: usize,
    is_cursor: bool,
    width: u16,
) -> Line<'static> {
    let Some(item) = items.get(idx) else {
        return Line::raw("");
    };

    let marker = if is_cursor { "\u{25b8} " } else { "  " };
    let label_style = if is_cursor {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let w = width as usize;

    // Confidence string: " 95.3%"
    let conf_str = format!(" {:.1}%", item.entry.confidence * 100.0);
    let conf_len = conf_str.len();

    // Recording title suffix (if available)
    let title_suffix = item
        .entry
        .recording_title
        .as_deref()
        .map(|t| format!("  {}", t))
        .unwrap_or_default();
    let title_suffix_len = title_suffix.chars().count();

    // Path gets remaining space, truncated left
    let path_max = w
        .saturating_sub(2) // marker
        .saturating_sub(conf_len)
        .saturating_sub(title_suffix_len);
    let path_display = truncate_left(&item.entry.display_name, path_max);

    let mut spans = vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(path_display, label_style),
        Span::styled(conf_str, Style::default().fg(Color::Yellow)),
    ];

    if !title_suffix.is_empty() {
        spans.push(Span::styled(
            title_suffix,
            Style::default().fg(Color::DarkGray),
        ));
    }

    Line::from(spans)
}
