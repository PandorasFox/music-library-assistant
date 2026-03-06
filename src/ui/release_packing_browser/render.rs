//! Rendering for the Release Packing Browser.
//!
//! Three-pane layout:
//! - Left (25%): flat release/near-miss/unmatched list
//! - Top-right (60%): tracks+unfilled for selected release
//! - Bottom-right (40%): per-track detail with score breakdown

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::types::*;
use super::ReleasePackingBrowserState;
use crate::ui::widgets::control_colors;

pub fn render(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    // Outer: info bar (1) + content (min) + controls (1)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_info_bar(f, outer[0], state);
    render_controls(f, outer[2]);

    // Content: left (25%) + right (75%)
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(25), Constraint::Percentage(75)])
        .split(outer[1]);

    // Right side: tracks (60%) + detail (40%)
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(panes[1]);

    render_left_pane(f, panes[0], state);
    render_tracks_pane(f, right[0], state);
    render_detail_pane(f, right[1], state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &ReleasePackingBrowserState) {
    let line = Line::from(vec![
        Span::styled(" ", Style::default()),
        Span::styled(
            format!("{}", state.fingerprinted_count),
            Style::default().fg(Color::White),
        ),
        Span::styled(" fingerprinted → ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", state.matched_count),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!(" matched ({} recordings) → ", state.recording_count),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(
            format!("{}", state.total_assigned),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!(" assigned to {} releases", state.total_releases),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(vec![line]), area);
}

fn render_controls(f: &mut Frame, area: Rect) {
    let line = Line::from(vec![
        control_colors::text(" "),
        control_colors::nav("^v"),
        control_colors::text(" nav  "),
        control_colors::nav("Shift+Arrow"),
        control_colors::text(" switch pane  "),
        control_colors::cancel("Esc"),
        control_colors::text(" close"),
    ]);
    f.render_widget(Paragraph::new(vec![line]), area);
}

// ============================================================================
// Left Pane: Flat Release List
// ============================================================================

fn render_left_pane(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    let focused = matches!(state.focused_pane, FocusedPane::LeftPane);
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };
    let block = Block::default()
        .title(" Releases ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    state.click_targets.clear();
    state.click_targets.set_list_area(inner);

    let mut lines: Vec<Line> = Vec::new();

    for (idx, entry) in state.entries.iter().enumerate() {
        let is_selected = focused && state.cursor == idx;
        let line = render_left_entry(entry, is_selected, state, inner.width as usize);
        let line_idx = lines.len();

        if !entry.is_section_header() {
            state
                .click_targets
                .add_row(idx.to_string(), inner.y + line_idx as u16);
        }

        lines.push(line);
    }

    // Scroll: ensure cursor is visible
    let visible_height = inner.height as usize;
    if visible_height > 0 && state.cursor >= state.scroll + visible_height {
        state.scroll = state.cursor - visible_height + 1;
    }
    if state.cursor < state.scroll {
        state.scroll = state.cursor;
    }

    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(state.scroll)
        .take(visible_height)
        .collect();
    f.render_widget(Paragraph::new(visible_lines), inner);
}

fn render_left_entry(
    entry: &PackingListEntry,
    selected: bool,
    state: &ReleasePackingBrowserState,
    width: usize,
) -> Line<'static> {
    let title_max = width.saturating_sub(10); // room for marker + coverage

    match entry {
        PackingListEntry::ReleaseSectionHeader { count } => Line::from(Span::styled(
            format!("── Releases ({}) ──", count),
            Style::default().fg(Color::DarkGray),
        )),

        PackingListEntry::ReleaseHeader { release_idx } => {
            let release = &state.releases[*release_idx];
            let marker = if selected { "▸ " } else { "  " };
            let coverage_color = if release.coverage >= 0.9 {
                Color::Green
            } else if release.coverage >= 0.7 {
                Color::Yellow
            } else {
                Color::Red
            };
            let title_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            let filled = release.tracks.len();
            let total = release.total_tracks;

            Line::from(vec![
                Span::styled(marker.to_string(), title_style),
                Span::styled(
                    truncate_for_width(&release.release_title, title_max),
                    title_style,
                ),
                Span::styled(
                    format!(" {}/{}", filled, total),
                    Style::default().fg(coverage_color),
                ),
            ])
        }

        PackingListEntry::SinglesSectionHeader { count } => Line::from(Span::styled(
            format!("── Singles ({}) ──", count),
            Style::default().fg(Color::DarkGray),
        )),

        PackingListEntry::SingleHeader { single_idx } => {
            let single = &state.singles[*single_idx];
            let marker = if selected { "▸ " } else { "  " };
            let title_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            Line::from(vec![
                Span::styled(marker.to_string(), title_style),
                Span::styled(
                    truncate_for_width(&single.release_title, title_max),
                    title_style,
                ),
                Span::styled(
                    " 1/1".to_string(),
                    Style::default().fg(Color::Green),
                ),
            ])
        }

        PackingListEntry::NearMissSectionHeader { count } => Line::from(Span::styled(
            format!("── Near-Misses ({}) ──", count),
            Style::default().fg(Color::DarkGray),
        )),

        PackingListEntry::NearMissEntry { idx } => {
            let nm = &state.near_misses[*idx];
            let marker = if selected { "▸ " } else { "  " };
            let label_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Yellow)
            };

            Line::from(vec![
                Span::styled(marker.to_string(), label_style),
                Span::styled("! ", Style::default().fg(Color::Yellow)),
                Span::styled(
                    truncate_for_width(&nm.release_title, title_max.saturating_sub(2)),
                    label_style,
                ),
                Span::styled(
                    format!(" {}/{}", nm.filled_count, nm.total_tracks),
                    Style::default().fg(Color::Yellow),
                ),
            ])
        }

        PackingListEntry::UnmatchedSectionHeader { count } => Line::from(Span::styled(
            format!("── Unmatched ({}) ──", count),
            Style::default().fg(Color::DarkGray),
        )),

        PackingListEntry::UnmatchedFile { idx } => {
            let um = &state.unmatched[*idx];
            let marker = if selected { "▸ " } else { "  " };
            let label_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            let filename = um.path.rsplit('/').next().unwrap_or(&um.path);

            Line::from(vec![
                Span::styled(marker.to_string(), label_style),
                Span::styled(
                    truncate_for_width(filename, title_max),
                    label_style,
                ),
            ])
        }
    }
}

// ============================================================================
// Middle Pane: Tracks for Selected Release
// ============================================================================

fn render_tracks_pane(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    let focused = matches!(state.focused_pane, FocusedPane::MiddlePane);
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };
    let block = Block::default()
        .title(" Tracks ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    // Determine which release to show (if any) before borrowing state mutably
    enum TrackSource {
        Release(usize),
        Single(usize),
        NearMiss(usize),
        Unmatched(usize),
        None,
    }

    let source = match state.selected_entry() {
        Some(PackingListEntry::ReleaseHeader { release_idx }) => TrackSource::Release(*release_idx),
        Some(PackingListEntry::SingleHeader { single_idx }) => TrackSource::Single(*single_idx),
        Some(PackingListEntry::NearMissEntry { idx }) => TrackSource::NearMiss(*idx),
        Some(PackingListEntry::UnmatchedFile { idx }) => TrackSource::Unmatched(*idx),
        _ => TrackSource::None,
    };

    match source {
        TrackSource::Release(idx) => {
            render_release_tracks(f, inner, state, idx, false);
        }
        TrackSource::Single(idx) => {
            render_release_tracks(f, inner, state, idx, true);
        }
        TrackSource::NearMiss(idx) => {
            let lines = render_near_miss_detail(&state.near_misses[idx]);
            render_scrollable_lines(f, inner, &lines, 0);
        }
        TrackSource::Unmatched(idx) => {
            let lines = render_unmatched_detail(&state.unmatched[idx]);
            render_scrollable_lines(f, inner, &lines, 0);
        }
        TrackSource::None => {
            let line = Line::from(Span::styled(
                "No release selected",
                Style::default().fg(Color::DarkGray),
            ));
            f.render_widget(Paragraph::new(vec![line]), inner);
        }
    }
}

fn render_release_tracks(
    f: &mut Frame,
    area: Rect,
    state: &mut ReleasePackingBrowserState,
    idx: usize,
    is_single: bool,
) {
    let focused = matches!(state.focused_pane, FocusedPane::MiddlePane);
    let release = if is_single { &state.singles[idx] } else { &state.releases[idx] };
    let track_count = release.tracks.len();
    let unfilled_count = release.unfilled.len();
    let total = track_count + unfilled_count;

    // Build merged list: tracks and unfilled slots sorted by position
    let mut lines: Vec<Line> = Vec::new();

    // Assigned tracks
    for (i, track) in release.tracks.iter().enumerate() {
        let is_selected = focused && state.track_cursor == i;
        let filename = track.path.rsplit('/').next().unwrap_or(&track.path);

        let label_style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };
        let marker = if is_selected { "▸ " } else { "  " };

        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), label_style),
            Span::styled(
                format!("{:>2} ", track.track_number),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                truncate_for_width(filename, 30),
                label_style,
            ),
            Span::styled(
                format!("  {:.2}", track.score),
                Style::default().fg(Color::Yellow),
            ),
        ]));
    }

    // Unfilled slots
    for (i, slot) in release.unfilled.iter().enumerate() {
        let idx = track_count + i;
        let is_selected = focused && state.track_cursor == idx;
        let label_style = if is_selected {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Red)
        };
        let marker = if is_selected { "▸ " } else { "  " };

        lines.push(Line::from(vec![
            Span::styled(marker.to_string(), label_style),
            Span::styled(
                format!("{:>2} ", slot.track_pos),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled("░ ", Style::default().fg(Color::Red)),
            Span::styled(
                truncate_for_width(&slot.track_title, 28),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
            Span::styled("  ----", Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Clamp track_cursor
    if total > 0 && state.track_cursor >= total {
        state.track_cursor = total - 1;
    }

    // Scroll: ensure track_cursor is visible
    let visible_height = area.height as usize;
    if visible_height > 0 && state.track_cursor >= state.track_scroll + visible_height {
        state.track_scroll = state.track_cursor - visible_height + 1;
    }
    if state.track_cursor < state.track_scroll {
        state.track_scroll = state.track_cursor;
    }

    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(state.track_scroll)
        .take(visible_height)
        .collect();
    f.render_widget(Paragraph::new(visible_lines), area);
}

// ============================================================================
// Detail Pane: Per-Track Detail
// ============================================================================

fn render_detail_pane(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    let focused = matches!(state.focused_pane, FocusedPane::DetailPane);
    let border_color = if focused { Color::Yellow } else { Color::DarkGray };
    let block = Block::default()
        .title(" Detail ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    let lines = match state.selected_entry() {
        Some(PackingListEntry::ReleaseHeader { release_idx }) => {
            detail_for_release(&state.releases[*release_idx], state.track_cursor)
        }
        Some(PackingListEntry::SingleHeader { single_idx }) => {
            detail_for_release(&state.singles[*single_idx], state.track_cursor)
        }
        Some(PackingListEntry::NearMissEntry { idx }) => {
            render_near_miss_detail(&state.near_misses[*idx])
        }
        Some(PackingListEntry::UnmatchedFile { idx }) => {
            render_unmatched_detail(&state.unmatched[*idx])
        }
        _ => vec![Line::from(Span::styled(
            "No selection",
            Style::default().fg(Color::DarkGray),
        ))],
    };

    render_scrollable_lines(f, inner, &lines, state.detail_scroll);

    // Clamp detail_scroll
    let total_lines = lines.len();
    let visible = inner.height as usize;
    if total_lines > visible {
        if state.detail_scroll > total_lines - visible {
            state.detail_scroll = total_lines - visible;
        }
    } else {
        state.detail_scroll = 0;
    }
}

fn detail_for_release(release: &ReleaseGroup, track_cursor: usize) -> Vec<Line<'static>> {
    let track_count = release.tracks.len();
    if track_cursor < track_count {
        render_track_detail(&release.tracks[track_cursor], release)
    } else if track_cursor < track_count + release.unfilled.len() {
        render_unfilled_detail(&release.unfilled[track_cursor - track_count], release)
    } else {
        render_release_summary(release)
    }
}

fn render_scrollable_lines(f: &mut Frame, area: Rect, lines: &[Line<'static>], scroll: usize) {
    let visible = area.height as usize;
    let visible_lines: Vec<Line> = lines
        .iter()
        .skip(scroll)
        .take(visible)
        .cloned()
        .collect();
    let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

// ============================================================================
// Detail Renderers (reused from original)
// ============================================================================

fn render_release_summary(release: &ReleaseGroup) -> Vec<Line<'static>> {
    vec![
        kv_line("Release:", &release.release_title),
        kv_line("Artist:", &release.release_artist),
        kv_line("MBID:", &release.release_id),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled("Coverage: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "{}/{} tracks ({:.0}%)",
                    release.tracks.len(),
                    release.total_tracks,
                    release.coverage * 100.0
                ),
                Style::default().fg(if release.coverage >= 0.9 {
                    Color::Green
                } else if release.coverage >= 0.7 {
                    Color::Yellow
                } else {
                    Color::Red
                }),
            ),
        ]),
    ]
}

fn render_track_detail(track: &AssignedTrackInfo, release: &ReleaseGroup) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Track Assignment",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        kv_line("File:", &track.path),
        Line::from(Span::raw("")),
        kv_line("Release:", &release.release_title),
        Line::from(vec![
            Span::styled("Position:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!(
                    "Disc {}, Track {} ({})",
                    track.medium_position,
                    track.track_position,
                    track.medium_format.as_deref().unwrap_or("?")
                ),
                Style::default().fg(Color::White),
            ),
        ]),
        kv_line("MB Track:", &track.track_title),
        kv_line("Recording:", &track.recording_id),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled("Score: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{:.3}", track.score),
                Style::default()
                    .fg(Color::Yellow)
                    .add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(Span::styled(
            "── Score Breakdown ──────────────────",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::raw("")),
    ];

    let breakdown = &track.score_breakdown;
    let scores = [
        ("AcoustID confidence:", breakdown.acoustid_confidence),
        ("Duration match:     ", breakdown.duration_match),
        ("Tag similarity:     ", breakdown.tag_similarity),
        ("Track number match: ", breakdown.track_number_match),
        ("Directory cohesion: ", breakdown.directory_cohesion),
    ];

    for (label, value) in &scores {
        lines.push(render_score_bar(label, *value));
    }

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::styled("Alternatives considered: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", track.alternatives_count),
            Style::default().fg(Color::White),
        ),
    ]));
    lines.push(Line::from(vec![
        Span::styled("Release coverage: ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "{:.0}% ({}/{})",
                release.coverage * 100.0,
                release.tracks.len(),
                release.total_tracks
            ),
            Style::default().fg(Color::White),
        ),
    ]));

    lines
}

fn render_unfilled_detail(slot: &UnfilledSlotInfo, release: &ReleaseGroup) -> Vec<Line<'static>> {
    let filled = release.tracks.len();
    let total = release.total_tracks;
    vec![
        Line::from(Span::styled(
            "Unfilled Release Slot",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        kv_line("Release:", &release.release_title),
        kv_line("Artist:", &release.release_artist),
        Line::from(vec![
            Span::styled("Position:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("Disc {}, Track {}", slot.medium_pos, slot.track_pos),
                Style::default().fg(Color::White),
            ),
        ]),
        kv_line("Expected:", &slot.track_title),
        kv_line("Recording:", &slot.recording_id),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "This track position has no matching corpus file.",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            format!(
                "{} of {} tracks are filled ({:.0}% coverage).",
                filled,
                total,
                release.coverage * 100.0
            ),
            Style::default().fg(Color::DarkGray),
        )),
    ]
}

fn render_near_miss_detail(nm: &crate::meta::signals::data::NearMissReleaseData) -> Vec<Line<'static>> {
    vec![
        Line::from(Span::styled(
            "Near-Miss Release",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        kv_line("Release:", &nm.release_title),
        kv_line("Artist:", &nm.release_artist),
        kv_line("Directory:", &nm.directory),
        Line::from(vec![
            Span::styled("Coverage:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{}/{} tracks", nm.filled_count, nm.total_tracks),
                Style::default().fg(Color::Yellow),
            ),
        ]),
        Line::from(Span::raw("")),
        kv_line("Candidate file:", &nm.candidate_path),
        Line::from(Span::raw("")),
        Line::from(vec![
            Span::styled("Missing slot: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("Disc {}, Track {}", nm.missing_medium_pos, nm.missing_track_pos),
                Style::default().fg(Color::White),
            ),
        ]),
        kv_line("Expected:", &nm.missing_track_title),
        kv_line("Recording:", &nm.missing_recording_id),
    ]
}

fn render_unmatched_detail(um: &UnmatchedEntry) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Unmatched Corpus Track",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        kv_line("File:", &um.path),
        Line::from(Span::raw("")),
    ];

    let rec_count = um.data.recording_ids.len();

    if rec_count > 0 {
        lines.push(Line::from(Span::styled(
            format!(
                "AcoustID matched {} recording{} but this file was not",
                rec_count,
                if rec_count == 1 { "" } else { "s" }
            ),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "assigned to any release during conflict resolution.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "No AcoustID recording matches found.",
            Style::default().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::from(Span::raw("")));

    if !um.data.recording_ids.is_empty() {
        lines.push(Line::from(Span::styled(
            "Matched recordings:",
            Style::default().fg(Color::DarkGray),
        )));
        for rec_id in &um.data.recording_ids {
            lines.push(Line::from(Span::styled(
                format!("  {}", rec_id),
                Style::default().fg(Color::White),
            )));
        }
    }

    if !um.data.considered_release_ids.is_empty() {
        let rel_count = um.data.considered_release_ids.len();
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            format!("Considered releases: {}", rel_count),
            Style::default().fg(Color::DarkGray),
        )));
        for rel_id in &um.data.considered_release_ids {
            lines.push(Line::from(Span::styled(
                format!("  {}", rel_id),
                Style::default().fg(Color::White),
            )));
        }
    }

    lines
}

// ============================================================================
// Helpers
// ============================================================================

fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<11}", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}

fn render_score_bar(label: &str, value: f64) -> Line<'static> {
    const BAR_WIDTH: usize = 20;
    let filled = ((value * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    let bar = format!("{}{}", "█".repeat(filled), " ".repeat(empty));

    Line::from(vec![
        Span::styled(format!("  {}", label), Style::default().fg(Color::DarkGray)),
        Span::styled(format!(" {:.2}  ", value), Style::default().fg(Color::White)),
        Span::styled(bar, Style::default().fg(Color::Yellow)),
    ])
}

/// Truncate a string to max_chars, appending "..." if truncated.
fn truncate_for_width(s: &str, max_chars: usize) -> String {
    let char_count = s.chars().count();
    if char_count <= max_chars {
        s.to_string()
    } else if max_chars <= 3 {
        s.chars().take(max_chars).collect()
    } else {
        let mut result: String = s.chars().take(max_chars - 3).collect();
        result.push_str("...");
        result
    }
}
