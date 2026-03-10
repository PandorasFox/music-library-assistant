//! Insights View Rendering
//!
//! Renders the insights view as a StandardList with wizard pane support.
//! Headers are rendered as styled separators; entries show label + count.
//! Z opens a detail pane for the selected entry (replacing the old 65/35 split).

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use super::{InsightListItem, InsightsViewState};

/// Render the full insights view (titlebar is rendered by render_app).
pub fn render_insights_view(f: &mut Frame, area: Rect, state: &mut InsightsViewState) {
    let busy = state.is_witch_busy();

    state.list.render(
        f,
        area,
        &state.flat_items,
        |idx, is_cursor, _is_selected, _width| {
            render_item(&state.flat_items, idx, is_cursor, busy)
        },
        "Insights",
        true, // always focused (it's the only pane)
    );
}

/// Render a single list item as a styled Line.
fn render_item(
    items: &[InsightListItem],
    idx: usize,
    is_cursor: bool,
    busy: bool,
) -> Line<'static> {
    match &items[idx] {
        InsightListItem::Header { title, .. } => {
            let style = if busy {
                Style::default().fg(Color::DarkGray)
            } else {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            };
            Line::from(Span::styled(format!("── {} ──", title), style))
        }
        InsightListItem::Entry { entry, .. } => {
            let base_color = if busy { Color::DarkGray } else { entry.color };

            let style = if is_cursor && !busy {
                Style::default()
                    .fg(Color::Black)
                    .bg(base_color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(base_color)
            };

            let prefix = if is_cursor && !busy { "▶ " } else { "  " };
            let text = match entry.count {
                Some(count) => format!("{}{}: {}", prefix, entry.label, count),
                None => format!("{}{}", prefix, entry.label),
            };

            Line::from(Span::styled(text, style))
        }
    }
}
