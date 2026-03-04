//! Rendering for the Release Packing Browser.
//!
//! Full-screen two-pane layout (35/65):
//! - Left: hierarchical navigable list (releases → tracks/slots, near-misses, unmatched)
//! - Right: context-sensitive detail pane with wrapping text

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Wrap},
    Frame,
};

use super::types::*;
use super::ReleasePackingBrowserState;

pub fn render(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    // Info bar (1 line) + content area
    let outer = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(area);

    render_info_bar(f, outer[0], state);

    // Two-pane horizontal split: 35% left, 65% right
    let panes = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(outer[1]);

    render_left_pane(f, panes[0], state);
    render_right_pane(f, panes[1], state);
}

fn render_info_bar(f: &mut Frame, area: Rect, state: &ReleasePackingBrowserState) {
    let summary = format!(
        " {} tracks assigned to {} releases | Esc to close | Shift+Arrow to switch panes",
        state.total_assigned, state.total_releases,
    );
    let line = Line::from(Span::styled(
        summary,
        Style::default().fg(Color::DarkGray),
    ));
    f.render_widget(Paragraph::new(vec![line]), area);
}

// ============================================================================
// Left Pane: Hierarchical List
// ============================================================================

fn render_left_pane(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    let border_color = if state.detail_focused {
        Color::DarkGray
    } else {
        Color::Yellow
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
        let is_selected = !state.detail_focused && state.cursor == idx;
        let line = render_list_entry(entry, is_selected, state);
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

fn render_list_entry(
    entry: &PackingListEntry,
    selected: bool,
    state: &ReleasePackingBrowserState,
) -> Line<'static> {
    match entry {
        PackingListEntry::ReleaseSectionHeader { count } => Line::from(Span::styled(
            format!("── Releases ({}) ──────────────", count),
            Style::default().fg(Color::DarkGray),
        )),

        PackingListEntry::ReleaseHeader {
            release_idx,
            expanded,
        } => {
            let release = &state.releases[*release_idx];
            let arrow = if *expanded { "▾" } else { "▸" };
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
                Span::styled(format!("{} ", arrow), Style::default().fg(Color::DarkGray)),
                Span::styled(
                    truncate_for_width(&release.release_title, 25),
                    title_style,
                ),
                Span::styled(
                    format!(" {}/{}", filled, total),
                    Style::default().fg(coverage_color),
                ),
            ])
        }

        PackingListEntry::AssignedTrack {
            release_idx,
            track_idx,
        } => {
            let track = &state.releases[*release_idx].tracks[*track_idx];
            let marker = if selected { "▸ " } else { "  " };
            let label_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };

            // Show tree connector
            let release = &state.releases[*release_idx];
            let is_last = *track_idx == release.tracks.len() - 1 && release.unfilled.is_empty();
            let connector = if is_last { "└ " } else { "├ " };

            let filename = track
                .path
                .rsplit('/')
                .next()
                .unwrap_or(&track.path);

            Line::from(vec![
                Span::styled(marker.to_string(), label_style),
                Span::styled(
                    format!("  {} ", connector),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:>2} ", track.track_number),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    truncate_for_width(filename, 20),
                    label_style,
                ),
                Span::styled(
                    format!(" {:.2}", track.score),
                    Style::default().fg(Color::Yellow),
                ),
            ])
        }

        PackingListEntry::UnfilledSlot {
            release_idx,
            slot_idx,
        } => {
            let slot = &state.releases[*release_idx].unfilled[*slot_idx];
            let marker = if selected { "▸ " } else { "  " };
            let is_last = *slot_idx == state.releases[*release_idx].unfilled.len() - 1;
            let connector = if is_last { "└ " } else { "├ " };
            let label_style = if selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::Red)
            };

            Line::from(vec![
                Span::styled(marker.to_string(), label_style),
                Span::styled(
                    format!("  {} ", connector),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled("░ ".to_string(), Style::default().fg(Color::Red)),
                Span::styled(
                    format!("{:02} ", slot.track_pos),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    truncate_for_width(&slot.track_title, 18),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
                Span::styled(" ----", Style::default().fg(Color::DarkGray)),
            ])
        }

        PackingListEntry::NearMissSectionHeader { count } => Line::from(Span::styled(
            format!("── Near-Misses ({}) ──────────", count),
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
                    truncate_for_width(&nm.release_title, 28),
                    label_style,
                ),
                Span::styled(
                    format!(" {}/{}", nm.filled_count, nm.total_tracks),
                    Style::default().fg(Color::Yellow),
                ),
            ])
        }

        PackingListEntry::UnmatchedSectionHeader { count } => Line::from(Span::styled(
            format!("── Unmatched ({}) ───────────", count),
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
                Span::styled(truncate_for_width(filename, 35).to_string(), label_style),
            ])
        }
    }
}

/// Truncate a string to max_chars, appending "..." if truncated.
/// Uses char boundaries for safety.
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

// ============================================================================
// Right Pane: Context-Sensitive Detail
// ============================================================================

fn render_right_pane(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
    let border_color = if state.detail_focused {
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
        Some(PackingListEntry::ReleaseHeader { release_idx, .. }) => {
            render_release_detail(&state.releases[*release_idx])
        }
        Some(PackingListEntry::AssignedTrack {
            release_idx,
            track_idx,
        }) => render_track_detail(
            &state.releases[*release_idx].tracks[*track_idx],
            &state.releases[*release_idx],
        ),
        Some(PackingListEntry::UnfilledSlot {
            release_idx,
            slot_idx,
        }) => render_unfilled_detail(
            &state.releases[*release_idx].unfilled[*slot_idx],
            &state.releases[*release_idx],
        ),
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

    let visible_lines: Vec<Line> = lines
        .into_iter()
        .skip(state.detail_scroll)
        .take(visible)
        .collect();

    let paragraph = Paragraph::new(visible_lines).wrap(Wrap { trim: false });
    f.render_widget(paragraph, inner);
}

fn render_release_detail(release: &ReleaseGroup) -> Vec<Line<'static>> {
    let mut lines = vec![
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
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "── Track List ──────────────────────",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::raw("")),
    ];

    // Track table
    for track in &release.tracks {
        let filename = track.path.rsplit('/').next().unwrap_or(&track.path);
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:>2} ", track.track_number),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<20} ", truncate_for_width(&track.track_title, 20)),
                Style::default().fg(Color::White),
            ),
            Span::styled(
                format!("{:<18} ", truncate_for_width(filename, 18)),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(format!("{:.2}", track.score), Style::default().fg(Color::Yellow)),
        ]));
    }

    // Unfilled slots
    for slot in &release.unfilled {
        lines.push(Line::from(vec![
            Span::styled(
                format!(" {:>2} ", slot.track_pos),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(
                format!("{:<20} ", truncate_for_width(&slot.track_title, 20)),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::ITALIC),
            ),
            Span::styled("(unfilled)", Style::default().fg(Color::Red)),
        ]));
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

    // Score breakdown bars
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
    let rel_count = um.data.considered_release_ids.len();

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

/// Key-value line: "Label:  value"
fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<11}", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}

/// Render a score bar: "  label  0.95  ████████████████████"
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
