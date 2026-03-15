//! Missing Album Singles Resolution Modal (V3)
//!
//! Single-load layout with StandardList + ButtonRow.

use std::collections::BTreeSet;

use ratatui::{
    layout::{Alignment, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear},
    Frame,
};

use mm_meta::domain_queries::MissingAlbumSingleSignalWire;
use mm_ui::rich_text::{RichBlock, RichSpan};
use mm_ui::standard_list::ListEntry;
use mm_ui::wizard::{WizardItem, WizardOffer};
use crate::helpers::truncate_right;

// ============================================================================
// V3: WizardItem wrapper for StandardList
// ============================================================================

/// Display wrapper for tracks in a Missing Album artist group (StandardList).
pub struct TrackListItem {
    pub title: String,
    pub path: String,
    pub inode: i64,
}

impl WizardItem for TrackListItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let content = vec![
            RichBlock::Paragraph(vec![RichSpan::new(
                &self.path,
                Style::default().fg(Color::White),
            )]),
        ];
        Some(WizardOffer::Pane {
            title: format!("Track: \"{}\"", self.title),
            content,
        })
    }
}

impl ListEntry for TrackListItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
}

/// Build items vec from a signal wire's tracks.
pub fn build_track_items(signal: &MissingAlbumSingleSignalWire) -> Vec<TrackListItem> {
    signal.data.tracks.iter().map(|t| TrackListItem {
        title: t.title.clone(),
        path: t.path.clone(),
        inode: t.inode,
    }).collect()
}

// ============================================================================
// V3: Render function
// ============================================================================

/// Render the Missing Album Singles resolution view.
///
/// Layout: Title (3) + StandardList with wizard (min) + Buttons (3)
pub fn render_v3(
    f: &mut Frame,
    area: Rect,
    data: &[MissingAlbumSingleSignalWire],
    current_group: usize,
    list: &mut mm_ui::standard_list::StandardListState,
    buttons: &mut mm_ui::modal_buttons::ButtonRowState<mm_ui::resolutions::missing_album::MissingAlbumButton>,
    focus: mm_ui::geometry::FocusPane,
    suffix: &str,
) {
    let padded = mm_ui::geometry::padded_rect(area);
    f.render_widget(Clear, padded);

    let vertical = ratatui::layout::Layout::default()
        .direction(ratatui::layout::Direction::Vertical)
        .constraints([
            ratatui::layout::Constraint::Length(3), // Title bar
            ratatui::layout::Constraint::Min(5),    // StandardList
            ratatui::layout::Constraint::Length(3),  // Buttons
        ])
        .split(padded);

    // --- Title bar ---
    render_missing_album_title(f, vertical[0], data, current_group, suffix);

    // --- StandardList ---
    let signal = data.get(current_group);
    let items: Vec<TrackListItem> = signal
        .map(build_track_items)
        .unwrap_or_default();

    let list_focused = focus == mm_ui::geometry::FocusPane::List;

    let list_title = {
        let current = current_group + 1;
        let total = data.len();
        let artist = signal.map(|s| s.data.artist.as_str()).unwrap_or("?");
        let track_count = items.len();
        format!("{} ({} tracks) [group {}/{}] \u{2014} [Z] details", artist, track_count, current, total)
    };

    crate::widgets::standard_list::render_standard_list(
        list,
        f,
        vertical[1],
        &items,
        |idx, is_cursor, _is_selected, width| render_track_item(&items, idx, is_cursor, width),
        &list_title,
        list_focused,
    );

    // --- Buttons ---
    let ctx = mm_ui::resolutions::missing_album::MissingAlbumButtonCtx {
        has_tracks: !items.is_empty(),
        group_index: current_group,
    };
    let button_focused = focus == mm_ui::geometry::FocusPane::Buttons;
    crate::widgets::modal_buttons::render_buttons(buttons, f, vertical[2], &ctx, button_focused);
}

fn render_missing_album_title(
    f: &mut Frame,
    area: Rect,
    data: &[MissingAlbumSingleSignalWire],
    current_group: usize,
    suffix: &str,
) {
    let signal = data.get(current_group);
    let current = current_group + 1;
    let total = data.len();
    let track_count = signal.map_or(0, |s| s.data.tracks.len());
    let artist = signal.map(|s| s.data.artist.as_str()).unwrap_or("?");

    let title = format!(
        " Missing Album: \"{}\" ({}/{}) \u{2014} {} tracks, suffix: \"{}\" ",
        artist, current, total, track_count, suffix,
    );

    let block = Block::default()
        .title(title)
        .title_alignment(Alignment::Center)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    f.render_widget(block, area);
}

fn render_track_item(
    items: &[TrackListItem],
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
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };

    let max_width = width.saturating_sub(2) as usize;
    let title_max = max_width.saturating_sub(10);
    let display = truncate_right(&item.title, title_max);

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(format!("\"{}\"", display), label_style),
        Span::styled(
            format!(" \u{2014} {}", truncate_right(&item.path, max_width.saturating_sub(title_max + 5))),
            Style::default().fg(Color::DarkGray),
        ),
    ])
}
