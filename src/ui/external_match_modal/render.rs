//! Rendering for the external match review modal.
//!
//! Read-only browser: track → MB recording URL + confidence.
//! Info bar (3 lines) + full-width StandardList with wizard popups/panes.

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::Frame;

use crate::ui::helpers::render_pane;
use crate::ui::widgets::{PathField, ResolutionLayout};

use super::types::ExternalMatchReviewState;

pub fn render(f: &mut Frame, area: Rect, state: &mut ExternalMatchReviewState) {
    let padded = ResolutionLayout::padded(area);
    f.render_widget(Clear, padded);

    // Two-row layout: info bar (3 lines) + list (rest)
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(5)])
        .split(padded);

    render_info_bar(f, vertical[0], state);

    state.list.render(
        f,
        vertical[1],
        &state.items,
        |idx, is_cursor, _is_selected, _width| render_item(&state.items, idx, is_cursor),
        "Files",
        true, // always focused
    );
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &ExternalMatchReviewState) {
    let title = format!(
        " External Match Browser \u{2014} {} file{} ",
        state.items.len(),
        if state.items.len() == 1 { "" } else { "s" },
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let inner = render_pane(f, area, block);

    if let Some(item) = state.items.get(state.list.cursor) {
        let lines = PathField::new(
            Span::styled("Path: ", Style::default().fg(Color::DarkGray)),
            &item.entry.path,
        )
        .style(Style::default().fg(Color::White))
        .render_lines(inner.width);
        let para = Paragraph::new(lines);
        f.render_widget(para, inner);
    }
}

fn render_item(
    items: &[super::types::MatchReviewItem],
    idx: usize,
    is_cursor: bool,
) -> Line<'static> {
    let Some(item) = items.get(idx) else {
        return Line::raw("");
    };

    let marker = if is_cursor { "▸ " } else { "  " };
    let label_style = if is_cursor {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    // Show filename (last component of path) + confidence
    let filename = item
        .entry
        .path
        .rsplit('/')
        .next()
        .unwrap_or(&item.entry.path);

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(filename.to_string(), label_style),
        Span::styled(
            format!("  {:.0}%", item.entry.confidence * 100.0),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}
