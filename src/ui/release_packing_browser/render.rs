//! Rendering for the Release Packing Browser.
//!
//! Three-pane layout:
//! - Left (25%): flat release/unmatched list
//! - Top-right (60%): tracks+unfilled for selected release
//! - Bottom-right (40%): per-track detail with score breakdown

use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::types::*;
use super::ReleasePackingBrowserState;
use crate::meta::signals::packing_category::PackingCategory;
use crate::ui::widgets::control_colors;

pub fn render(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    // Outer: title (1) + content (min) + controls (1)
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(0),
            Constraint::Length(1),
        ])
        .split(area);

    render_title_bar(f, outer[0], state);
    let has_release = state.pin_input.is_none() && state.selected_release().is_some();
    render_controls(f, outer[2], has_release);

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

    // Pin input overlay on top of everything
    if state.pin_input.is_some() {
        render_pin_overlay(f, area, state);
    }
}

fn render_title_bar(f: &mut Frame, area: Rect, state: &ReleasePackingBrowserState) {
    let title = state.category.label();
    let count = state.entries.len();
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", title),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!("({} entries)", count),
            Style::default().fg(Color::DarkGray),
        ),
    ]);
    f.render_widget(Paragraph::new(vec![line]), area);
}

fn render_controls(f: &mut Frame, area: Rect, has_release: bool) {
    let mut spans = vec![
        control_colors::text(" "),
        control_colors::nav("^v"),
        control_colors::text(" nav  "),
        control_colors::nav("Shift+Arrow"),
        control_colors::text(" switch pane  "),
    ];
    if has_release {
        spans.push(control_colors::nav("p"));
        spans.push(control_colors::text(" pin release  "));
    }
    spans.push(control_colors::cancel("Esc"));
    spans.push(control_colors::text(" close"));
    let line = Line::from(spans);
    f.render_widget(Paragraph::new(vec![line]), area);
}

// ============================================================================
// Left Pane: Flat Release List
// ============================================================================

fn render_left_pane(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    let focused = matches!(state.focused_pane, FocusedPane::LeftPane);
    let border_color = if focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };
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

        state
            .click_targets
            .add_row(idx.to_string(), inner.y + line_idx as u16);

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
    let marker = if selected { "▸ " } else { "  " };

    match entry {
        PackingListEntry::Release { idx } => {
            let release = &state.releases[*idx];
            let title_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let coverage_color = coverage_color(release, state.category);

            let mut spans = vec![
                Span::styled(marker.to_string(), title_style),
                Span::styled(
                    truncate_for_width(&release.release_title, title_max),
                    title_style,
                ),
                Span::styled(
                    format!(" {}/{}", release.tracks.len(), release.total_tracks),
                    Style::default().fg(coverage_color),
                ),
            ];
            if !release.alternatives.is_empty() {
                spans.push(Span::styled(
                    format!(" +{}", release.alternatives.len()),
                    Style::default().fg(Color::DarkGray),
                ));
            }
            if release.va_override.is_some() {
                spans.push(Span::styled(
                    " VA".to_string(),
                    Style::default().fg(Color::Yellow),
                ));
            }
            Line::from(spans)
        }

        PackingListEntry::Unmatched { idx } => {
            let um = &state.unmatched[*idx];
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
                Span::styled(truncate_for_width(filename, title_max), label_style),
            ])
        }
    }
}

// ============================================================================
// Middle Pane: Tracks for Selected Release
// ============================================================================

fn render_tracks_pane(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    let focused = matches!(state.focused_pane, FocusedPane::MiddlePane);
    let border_color = if focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    let block = Block::default()
        .title(" Tracks ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color));
    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    match state.selected_entry() {
        Some(PackingListEntry::Release { idx }) => {
            render_release_tracks(f, inner, state, *idx);
        }
        Some(PackingListEntry::Unmatched { idx }) => {
            let lines = render_unmatched_detail(&state.unmatched[*idx]);
            render_scrollable_lines(f, inner, &lines, 0);
        }
        None => {
            let line = Line::from(Span::styled(
                "No entries",
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
) {
    let focused = matches!(state.focused_pane, FocusedPane::MiddlePane);
    let release = &state.releases[idx];
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
            Span::styled(truncate_for_width(filename, 30), label_style),
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
    let border_color = if focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };
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
        Some(PackingListEntry::Release { idx }) => {
            if matches!(state.focused_pane, FocusedPane::LeftPane) {
                render_release_overview(&state.releases[*idx], state.category)
            } else {
                detail_for_release(&state.releases[*idx], state.track_cursor, state.category)
            }
        }
        Some(PackingListEntry::Unmatched { idx }) => {
            render_unmatched_detail(&state.unmatched[*idx])
        }
        None => vec![Line::from(Span::styled(
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

fn detail_for_release(
    release: &ReleaseGroup,
    track_cursor: usize,
    category: PackingCategory,
) -> Vec<Line<'static>> {
    let track_count = release.tracks.len();
    if track_cursor < track_count {
        render_track_detail(&release.tracks[track_cursor], release)
    } else if track_cursor < track_count + release.unfilled.len() {
        render_unfilled_detail(&release.unfilled[track_cursor - track_count], release)
    } else {
        render_release_summary(release, category)
    }
}

fn render_scrollable_lines(f: &mut Frame, area: Rect, lines: &[Line<'static>], scroll: usize) {
    let visible = area.height as usize;
    let visible_lines: Vec<Line> = lines.iter().skip(scroll).take(visible).cloned().collect();
    let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

// ============================================================================
// Detail Renderers (reused from original)
// ============================================================================

fn render_release_summary(release: &ReleaseGroup, category: PackingCategory) -> Vec<Line<'static>> {
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
                Style::default().fg(coverage_color(release, category)),
            ),
        ]),
    ]
}

fn render_release_overview(release: &ReleaseGroup, category: PackingCategory) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Low confidence warning banner — bright and prominent
    if let Some(ref reason) = release.low_confidence_reason {
        lines.push(Line::from(Span::styled(
            "⚠ LOW CONFIDENCE — PROBABLE MISPACK ⚠",
            Style::default()
                .fg(Color::Red)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(vec![
            Span::styled("  AcoustID ratio: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{:.1}%", reason.acoustid_ratio * 100.0),
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" below threshold", Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Avg album match: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                format!("{:.2}", reason.avg_album_match),
                Style::default()
                    .fg(Color::Red)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(" below threshold", Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(Span::raw("")));
    }

    lines.extend([
        Line::from(Span::styled(
            "Release Overview",
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        kv_line("Release:", &release.release_title),
        kv_line("Artist:", &release.release_artist),
        kv_line("MBID:", &release.release_id),
        Line::from(Span::raw("")),
    ]);

    // Coverage
    let cov_color = coverage_color(release, category);
    lines.push(Line::from(vec![
        Span::styled("Coverage:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(
                "{}/{} tracks ({:.0}%)",
                release.tracks.len(),
                release.total_tracks,
                release.coverage * 100.0
            ),
            Style::default().fg(cov_color),
        ),
    ]));

    // File type breakdown from assigned tracks
    let mut ext_counts: HashMap<String, usize> = HashMap::new();
    let mut dir_counts: HashMap<String, usize> = HashMap::new();

    for track in &release.tracks {
        // Extension
        let ext = track
            .path
            .rsplit('.')
            .next()
            .unwrap_or("?")
            .to_lowercase();
        *ext_counts.entry(ext).or_default() += 1;

        // Parent directory
        let dir = match track.path.rsplit_once('/') {
            Some((parent, _)) => parent.to_string(),
            None => "?".to_string(),
        };
        *dir_counts.entry(dir).or_default() += 1;
    }

    // Format file types inline: "37 flac, 2 png"
    let mut ext_pairs: Vec<_> = ext_counts.into_iter().collect();
    ext_pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let ext_summary: String = ext_pairs
        .iter()
        .map(|(ext, count)| format!("{} {}", count, ext))
        .collect::<Vec<_>>()
        .join(", ");

    lines.push(Line::from(vec![
        Span::styled("Files:     ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{} assigned", release.tracks.len()),
            Style::default().fg(Color::White),
        ),
        Span::styled(
            format!(" ({})", ext_summary),
            Style::default().fg(Color::DarkGray),
        ),
    ]));

    if !release.unfilled.is_empty() {
        lines.push(Line::from(vec![
            Span::styled("Unfilled:  ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} slots", release.unfilled.len()),
                Style::default().fg(Color::Red),
            ),
        ]));
    }

    // Source directories
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(Span::styled(
        "── Source Directories ───────────────",
        Style::default().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::raw("")));

    let mut dir_pairs: Vec<_> = dir_counts.into_iter().collect();
    dir_pairs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    for (dir, count) in &dir_pairs {
        lines.push(Line::from(vec![
            Span::styled(
                format!("  {:>3}  ", count),
                Style::default().fg(Color::Yellow),
            ),
            Span::styled(dir.clone(), Style::default().fg(Color::White)),
        ]));
    }

    // VA Override
    if let Some(va) = &release.va_override {
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "── VA Override ─────────────────────",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(vec![
            Span::styled("  Suggested: ", Style::default().fg(Color::Yellow)),
            Span::styled(
                va.suggested_artist.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled("  Source:    ", Style::default().fg(Color::DarkGray)),
            Span::styled(va.source.clone(), Style::default().fg(Color::DarkGray)),
        ]));
    }

    // Alternatives
    if !release.alternatives.is_empty() {
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            format!(
                "── Alternatives ({}) ────────────────",
                release.alternatives.len()
            ),
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::raw("")));

        for (i, alt) in release.alternatives.iter().enumerate() {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("  {}. ", i + 1),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(alt.release_title.clone(), Style::default().fg(Color::White)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("     Artist: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    alt.release_artist.clone(),
                    Style::default().fg(Color::White),
                ),
                Span::styled("  Score: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    format!("{:.2}/{:.2}", alt.alternative_score, alt.winner_score),
                    Style::default().fg(Color::White),
                ),
                Span::styled(
                    format!("  ({} tracks)", alt.inode_count),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
            lines.push(Line::from(vec![
                Span::styled("     MBID: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    alt.release_id.clone(),
                    Style::default().fg(Color::DarkGray),
                ),
            ]));
        }
    }

    lines
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
        ("Title match:        ", breakdown.title_match),
        ("Artist match:       ", breakdown.artist_match),
        ("Album match:        ", breakdown.album_match),
        ("Track number match: ", breakdown.track_number_match),
    ];

    for (label, value) in &scores {
        lines.push(render_score_bar(label, *value));
    }

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        Span::styled(
            "Alternatives considered: ",
            Style::default().fg(Color::DarkGray),
        ),
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
// Pin Release Overlay
// ============================================================================

fn render_pin_overlay(f: &mut Frame, area: Rect, state: &ReleasePackingBrowserState) {
    let input: &crate::ui::widgets::TextInputState = match &state.pin_input {
        Some(input) => input,
        None => return,
    };

    // Centered popup: 60 wide, 7 tall (or 8 with error)
    let has_error = state.pin_error.is_some();
    let height = if has_error { 8 } else { 7 };
    let width = 60u16.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    // Clear background
    let clear = Paragraph::new(vec![Line::from(""); height as usize])
        .style(Style::default().bg(Color::Black));
    f.render_widget(clear, popup_area);

    let block = Block::default()
        .title(" Pin MusicBrainz Release ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    // Release ID label + text input
    let (before, cursor_char, after) = input.cursor_splits();
    lines.push(Line::from(vec![
        Span::styled("Release ID: ", Style::default().fg(Color::DarkGray)),
        Span::styled(before.to_string(), Style::default().fg(Color::White)),
        Span::styled(
            cursor_char.to_string(),
            Style::default().fg(Color::Black).bg(Color::White),
        ),
        Span::styled(after.to_string(), Style::default().fg(Color::White)),
    ]));

    // Error message if present
    if let Some(ref err) = state.pin_error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Red),
        )));
    }

    // Spacer + hint
    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(vec![
        control_colors::nav("Enter"),
        control_colors::text(" confirm  "),
        control_colors::cancel("Esc"),
        control_colors::text(" cancel"),
    ]));

    f.render_widget(Paragraph::new(lines), inner);
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
        Span::styled(
            format!(" {:.2}  ", value),
            Style::default().fg(Color::White),
        ),
        Span::styled(bar, Style::default().fg(Color::Yellow)),
    ])
}

/// Coverage color: yellow for LowConfidence (never green), normal thresholds otherwise.
fn coverage_color(release: &ReleaseGroup, category: PackingCategory) -> Color {
    if category == PackingCategory::LowConfidence {
        // Low confidence releases should never look healthy
        if release.coverage >= 0.7 {
            Color::Yellow
        } else {
            Color::Red
        }
    } else if release.coverage >= 1.0 {
        Color::Green
    } else if release.coverage >= 0.7 {
        Color::Yellow
    } else {
        Color::Red
    }
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
