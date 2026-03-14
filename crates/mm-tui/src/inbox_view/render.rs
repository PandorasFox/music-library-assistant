//! Inbox View Rendering
//!
//! Renders the inbox view as a StandardList.
//! Bucket entries show count + label with color coding.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use crate::widgets::standard_list::render_standard_list;

use super::{InboxInsightAction, InboxViewData, InboxInteraction};

/// Render the full inbox view (titlebar is rendered by render_app).
pub fn render_inbox_view(
    f: &mut Frame,
    area: Rect,
    data: &InboxViewData,
    interaction: &mut InboxInteraction,
) {
    let busy = data.busy;

    render_standard_list(
        &mut interaction.list,
        f,
        area,
        &data.entries,
        |idx, is_cursor, _is_selected, _width| {
            render_item(&data.entries, idx, is_cursor, busy)
        },
        "Inbox Overview",
        true, // always focused (only pane)
    );
}

/// Render a single inbox bucket entry as a styled Line.
fn render_item(
    entries: &[super::InboxBucketEntry],
    idx: usize,
    is_cursor: bool,
    busy: bool,
) -> Line<'static> {
    let entry = &entries[idx];
    let count_str = format!("{:>6}", entry.count);

    let (entry_color, label_style, arrow) = if busy {
        (Color::DarkGray, Style::default().fg(Color::DarkGray), " ")
    } else {
        (
            entry.color,
            if is_cursor {
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Gray)
            },
            if is_cursor { "▸" } else { " " },
        )
    };

    let action_indicator = if busy {
        ""
    } else {
        match entry.action {
            InboxInsightAction::LaunchIntake
            | InboxInsightAction::LaunchCorpusMatchResolution
            | InboxInsightAction::LaunchInboxTagCanonicity
            | InboxInsightAction::LaunchOrganize
            | InboxInsightAction::LaunchInboxCompoundSplit => " \u{23CE}",
            InboxInsightAction::Informational => "",
        }
    };

    Line::from(vec![
        Span::styled(format!("  {} ", arrow), Style::default().fg(entry_color)),
        Span::styled(
            format!("{} ", count_str),
            Style::default()
                .fg(entry_color)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(entry.label.clone(), label_style),
        Span::styled(action_indicator, Style::default().fg(Color::DarkGray)),
    ])
}
