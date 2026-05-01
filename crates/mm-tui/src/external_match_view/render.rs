//! Rendering for the External Matches lateral view.
//!
//! Full-width StandardList with wizard popup support.
//! Section headers are non-selectable; entries show icon + label + status/count.
//! Z opens wizard popup with detail for the selected entry.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    Frame,
};

use super::{ExternalMatchListItem, ExternalMatchesViewState, NavigableEntry};
use mm_meta::views::ConfidenceTier;
use mm_meta::signals::packing_category::PackingCategory;
use crate::widgets::standard_list::render_standard_list;

pub(crate) fn render(f: &mut Frame, area: Rect, state: &mut ExternalMatchesViewState) {
    // Snapshot fields for the render closure (avoids borrowing all of `data`
    // while `list` is mutably borrowed by render_standard_list).
    let snap = RenderSnapshot {
        has_api_key: state.data.has_api_key,
        fetch_active: state.data.fetch_active,
        fetch_progress: state.data.fetch_progress.as_ref(),
        cached_data: state.data.cached_data.as_ref(),
        cover_art_active: state.data.cover_art_active,
        cover_art_progress: state.data.cover_art_progress.as_ref(),
        deezer_active: state.data.deezer_active,
        deezer_progress: state.data.deezer_progress.as_ref(),
    };

    let flat_items = &state.data.flat_items;

    render_standard_list(
        &mut state.interaction.list,
        f,
        area,
        flat_items,
        |idx, is_cursor, _is_selected, _width| {
            render_item(flat_items, idx, is_cursor, &snap)
        },
        "Ext. Authorities",
        true, // always focused (only pane)
    );
}

/// Read-only snapshot of state fields needed by the render closure.
struct RenderSnapshot<'a> {
    has_api_key: bool,
    fetch_active: bool,
    fetch_progress: Option<&'a mm_meta::witch_types::FetchProgress>,
    cached_data: Option<&'a mm_meta::views::ExternalMatchesData>,
    cover_art_active: bool,
    cover_art_progress: Option<&'a mm_meta::witch_types::CoverArtProgress>,
    deezer_active: bool,
    deezer_progress: Option<&'a mm_meta::witch_types::DeezerProgress>,
}

fn render_item(
    items: &[ExternalMatchListItem],
    idx: usize,
    is_cursor: bool,
    snap: &RenderSnapshot,
) -> Line<'static> {
    match &items[idx] {
        ExternalMatchListItem::Header(title) => Line::from(Span::styled(
            format!("── {} ──────────────────", title),
            Style::default().fg(Color::DarkGray),
        )),

        ExternalMatchListItem::Spacer => Line::from(""),

        ExternalMatchListItem::InfoLine(line) => line.clone(),

        ExternalMatchListItem::Entry { nav, .. } => match nav {
            NavigableEntry::FetchAction => render_fetch_line(is_cursor, snap),
            NavigableEntry::PackReleasesAction => render_pack_releases_line(is_cursor, snap),
            NavigableEntry::CoverArtAction => render_cover_art_line(is_cursor, snap),
            NavigableEntry::DeezerArtAction => render_deezer_art_line(is_cursor, snap),
            NavigableEntry::UntaggedMatches => {
                let count = snap
                    .cached_data
                    .map(|d| d.untagged_entries.len())
                    .unwrap_or(0);
                render_bucket_line(
                    is_cursor,
                    "?",
                    Color::Magenta,
                    "Untagged matches",
                    count,
                    false,
                )
            }
            NavigableEntry::ConfidenceBucket(tier) => {
                let count = snap
                    .cached_data
                    .and_then(|d| d.confidence_buckets.iter().find(|b| b.tier == *tier))
                    .map(|b| b.total)
                    .unwrap_or(0);
                let (marker_color, dim) = match tier {
                    ConfidenceTier::Perfect | ConfidenceTier::VeryHigh | ConfidenceTier::High => {
                        (Color::Yellow, false)
                    }
                    ConfidenceTier::Medium => (Color::Yellow, true),
                    ConfidenceTier::Low => (Color::DarkGray, false),
                };
                let label = format!("{} confidence", tier.label());
                render_bucket_line(is_cursor, "!", marker_color, &label, count, dim)
            }
            NavigableEntry::PackingCategory(cat) => {
                let (icon, color, label, count) = packing_entry_info(*cat, snap);
                render_packing_line(is_cursor, icon, color, label, count)
            }
        },
    }
}

fn render_fetch_line(is_cursor: bool, snap: &RenderSnapshot) -> Line<'static> {
    let (status_label, status_color) = if !snap.has_api_key {
        ("No API Key".to_string(), Color::Red)
    } else if snap.fetch_active {
        if let Some(p) = &snap.fetch_progress {
            let a = &p.acoustid;
            let m = &p.mb;
            if m.total > 0 && a.total > 0 {
                (
                    format!(
                        "{}/{} + MB {}/{}",
                        a.processed, a.total, m.processed, m.total
                    ),
                    Color::Yellow,
                )
            } else if m.total > 0 {
                (format!("MB {}/{}", m.processed, m.total), Color::Yellow)
            } else {
                (format!("{}/{}", a.processed, a.total), Color::Yellow)
            }
        } else {
            ("Active".to_string(), Color::Yellow)
        }
    } else {
        ("Idle".to_string(), Color::Green)
    };

    let (marker, label_style) = cursor_marker_style(is_cursor, !snap.has_api_key || snap.fetch_active);

    Line::from(vec![
        Span::styled(marker, label_style),
        Span::styled("Cache external metadata matches  ", label_style),
        Span::styled(
            format!("{:<12}", status_label),
            Style::default().fg(status_color),
        ),
    ])
}

fn render_pack_releases_line(is_cursor: bool, snap: &RenderSnapshot) -> Line<'static> {
    let has_data = snap.cached_data.is_some();
    let stale = snap
        .cached_data
        .is_some_and(|d| d.pinned_releases_stale);
    let (status_label, status_color) = if snap.fetch_active {
        ("Fetch active", Color::DarkGray)
    } else if stale {
        ("Stale", Color::Yellow)
    } else if !has_data {
        ("No data", Color::DarkGray)
    } else {
        ("Ready", Color::Green)
    };

    let (marker, label_style) = cursor_marker_style(is_cursor, snap.fetch_active || !has_data);

    Line::from(vec![
        Span::styled(marker, label_style),
        Span::styled("Run release packing        ", label_style),
        Span::styled(
            format!("{:<12}", status_label),
            Style::default().fg(status_color),
        ),
    ])
}

fn render_cover_art_line(is_cursor: bool, snap: &RenderSnapshot) -> Line<'static> {
    let (status_label, status_color) = if snap.cover_art_active {
        if let Some(p) = &snap.cover_art_progress {
            (
                format!(
                    "{}/{} releases, {} images",
                    p.processed, p.total_releases, p.images_written
                ),
                Color::Yellow,
            )
        } else {
            ("Active".to_string(), Color::Yellow)
        }
    } else {
        ("Idle".to_string(), Color::Green)
    };

    let disabled = snap.cover_art_active;
    let (marker, label_style) = cursor_marker_style(is_cursor, disabled);

    Line::from(vec![
        Span::styled(marker, label_style),
        Span::styled("Download cover art            ", label_style),
        Span::styled(
            format!("{:<12}", status_label),
            Style::default().fg(status_color),
        ),
    ])
}

fn render_deezer_art_line(is_cursor: bool, snap: &RenderSnapshot) -> Line<'static> {
    let (status_label, status_color) = if snap.deezer_active {
        if let Some(p) = &snap.deezer_progress {
            (
                format!(
                    "{}/{} dirs, {} images",
                    p.processed, p.total_dirs, p.images_written
                ),
                Color::Yellow,
            )
        } else {
            ("Active".to_string(), Color::Yellow)
        }
    } else {
        ("Idle".to_string(), Color::Green)
    };

    let disabled = snap.deezer_active;
    let (marker, label_style) = cursor_marker_style(is_cursor, disabled);

    Line::from(vec![
        Span::styled(marker, label_style),
        Span::styled("Fetch cover art via Deezer    ", label_style),
        Span::styled(
            format!("{:<12}", status_label),
            Style::default().fg(status_color),
        ),
    ])
}

fn render_bucket_line(
    is_cursor: bool,
    marker_char: &str,
    marker_color: Color,
    label: &str,
    count: usize,
    dim: bool,
) -> Line<'static> {
    let (cursor_marker, label_style) = cursor_marker_style(is_cursor, dim);

    Line::from(vec![
        Span::styled(cursor_marker.to_string(), label_style),
        Span::styled(marker_char.to_string(), Style::default().fg(marker_color)),
        Span::styled(format!(" {:<24}", label), label_style),
        Span::styled(format!("{:>6}", count), Style::default().fg(Color::Yellow)),
    ])
}

fn render_packing_line(
    is_cursor: bool,
    icon: &str,
    color: Color,
    label: &str,
    count: usize,
) -> Line<'static> {
    let (marker, label_style) = cursor_marker_style(is_cursor, false);

    Line::from(vec![
        Span::styled(marker.to_string(), label_style),
        Span::styled(format!("{} ", icon), Style::default().fg(color)),
        Span::styled(format!("{:<24}", label), label_style),
        Span::styled(format!("{:>6}", count), Style::default().fg(color)),
    ])
}

/// Compute cursor marker ("▸ " or "  ") and label style for list rows.
///
/// Cursor row is always Cyan+Bold. Non-cursor: DarkGray if `dim`, White otherwise.
fn cursor_marker_style(is_cursor: bool, dim: bool) -> (&'static str, Style) {
    let marker = if is_cursor { "▸ " } else { "  " };
    let style = if is_cursor {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    } else if dim {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };
    (marker, style)
}

fn packing_entry_info(
    cat: PackingCategory,
    snap: &RenderSnapshot,
) -> (&'static str, Color, &'static str, usize) {
    let data = snap.cached_data;
    match cat {
        PackingCategory::Perfect => (
            "★",
            Color::Green,
            "Perfect",
            data.map(|d| d.packing_perfect_count).unwrap_or(0),
        ),
        PackingCategory::FullMatches => (
            "✓",
            Color::Green,
            "Full matches",
            data.map(|d| d.packing_full_match_count).unwrap_or(0),
        ),
        PackingCategory::Singles => (
            "♪",
            Color::Cyan,
            "Singles",
            data.map(|d| d.packing_singles_count).unwrap_or(0),
        ),
        PackingCategory::Incomplete => (
            "◐",
            Color::Yellow,
            "Incomplete",
            data.map(|d| d.packing_incomplete_count).unwrap_or(0),
        ),
        PackingCategory::LowConfidence => (
            "⚠",
            Color::Yellow,
            "Low confidence",
            data.map(|d| d.packing_low_confidence_count).unwrap_or(0),
        ),
        PackingCategory::Knots => (
            "⊛",
            Color::Red,
            "Knots",
            data.map(|d| d.packing_knots_count).unwrap_or(0),
        ),
        PackingCategory::UnsolvedConflict => (
            "✗",
            Color::Magenta,
            "Unsolved (conflict)",
            data.map(|d| d.unsolved_conflict_count).unwrap_or(0),
        ),
        PackingCategory::UnsolvedNoRelease => (
            "?",
            Color::DarkGray,
            "Unsolved (no release)",
            data.map(|d| d.unsolved_no_release_count).unwrap_or(0),
        ),
        PackingCategory::UnsolvedNoMatch => (
            "·",
            Color::DarkGray,
            "Unsolved (no match)",
            data.map(|d| d.unsolved_no_match_count).unwrap_or(0),
        ),
    }
}
