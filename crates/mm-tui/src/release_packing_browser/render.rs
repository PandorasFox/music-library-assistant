//! Rendering for the Release Packing Browser.
//!
//! Single-pane StandardList with wizard integration:
//! - List rows: checkbox + marker + title + coverage
//! - Wizard popup (z): release overview
//! - Wizard pane (Z): interleaved tracks + score breakdown cards
//! - Pin overlay: UUID input for manual release pinning

use std::collections::HashMap;

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::types::*;
use super::ReleasePackingBrowserState;
use mm_meta::signals::packing_category::PackingCategory;
use crate::widgets::control_colors;
use crate::widgets::rich_text::{RichBlock, RichSpan};

// ============================================================================
// Top-Level Render
// ============================================================================

pub(crate) fn render(f: &mut Frame, area: Rect, state: &mut ReleasePackingBrowserState) {
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
    let is_release_category = matches!(
        state.category,
        PackingCategory::Perfect
            | PackingCategory::FullMatches
            | PackingCategory::Singles
            | PackingCategory::Incomplete
            | PackingCategory::LowConfidence
    );
    render_controls(f, outer[2], has_release, is_release_category);

    // StandardList renders the list + wizard popup/pane
    let entries = &state.entries;
    let releases = &state.releases;
    let unmatched = &state.unmatched;
    let category = state.category;
    let pinned = &state.pinned_release_ids;

    state.list_state.render(
        f,
        outer[1],
        entries,
        |idx, is_cursor, is_selected, width| {
            render_list_row(
                &entries[idx],
                is_cursor,
                is_selected,
                width,
                releases,
                unmatched,
                category,
                pinned,
            )
        },
        category.label(),
        true,
    );

    // Pin input overlay on top of everything
    if state.pin_input.is_some() {
        render_pin_overlay(f, area, state);
    }
}

// ============================================================================
// Title Bar & Controls
// ============================================================================

fn render_title_bar(f: &mut Frame, area: Rect, state: &ReleasePackingBrowserState) {
    let title = state.category.label();
    let count = state.entries.len();
    let selected_count = state.list_state.selected.len();

    let mut spans = vec![
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
    ];

    if selected_count > 0 {
        spans.push(Span::styled(
            format!("  [{} selected]", selected_count),
            Style::default().fg(Color::Cyan),
        ));
    }

    f.render_widget(Paragraph::new(vec![Line::from(spans)]), area);
}

fn render_controls(f: &mut Frame, area: Rect, has_release: bool, is_release_category: bool) {
    let mut spans = vec![
        control_colors::text(" "),
        control_colors::nav("^v"),
        control_colors::text(" nav  "),
    ];
    if is_release_category {
        spans.push(control_colors::nav("Space"));
        spans.push(control_colors::text(" toggle  "));
        spans.push(control_colors::confirm("Enter"));
        spans.push(control_colors::text(" approve  "));
    }
    spans.push(control_colors::nav("z/Z"));
    spans.push(control_colors::text(" info  "));
    if has_release {
        spans.push(control_colors::nav("p"));
        spans.push(control_colors::text(" pin  "));
    }
    spans.push(control_colors::cancel("Esc"));
    spans.push(control_colors::text(" close"));
    f.render_widget(Paragraph::new(vec![Line::from(spans)]), area);
}

// ============================================================================
// List Row Rendering (closure for StandardList)
// ============================================================================

fn render_list_row(
    entry: &PackingListEntry,
    is_cursor: bool,
    is_selected: bool,
    width: u16,
    releases: &[ReleaseGroup],
    unmatched: &[UnmatchedEntry],
    category: PackingCategory,
    pinned: &std::collections::HashSet<String>,
) -> Line<'static> {
    let w = width as usize;

    match entry {
        PackingListEntry::Release { idx, .. } => {
            let release = &releases[*idx];
            let title_style = if is_cursor {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::White)
            };
            let coverage_color = coverage_color(release, category);

            // Checkbox + marker
            let checkbox = if is_selected { "[x] " } else { "[ ] " };
            let marker = if is_cursor { "▸ " } else { "  " };
            let prefix_len = 4 + 2; // checkbox + marker
            let title_max = w.saturating_sub(prefix_len + 10); // room for coverage+badges

            let mut spans = vec![
                Span::styled(
                    checkbox,
                    if is_selected {
                        Style::default().fg(Color::Green)
                    } else {
                        Style::default().fg(Color::DarkGray)
                    },
                ),
                Span::styled(marker.to_string(), title_style),
                Span::styled(
                    crate::helpers::truncate_right(&release.release_title, title_max),
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
            if pinned.contains(&release.release_id) {
                spans.push(Span::styled(
                    " PIN",
                    Style::default().fg(Color::LightBlue),
                ));
            }
            Line::from(spans)
        }

        PackingListEntry::Unmatched { idx, .. } => {
            let um = &unmatched[*idx];
            let label_style = if is_cursor {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let filename = um.path.rsplit('/').next().unwrap_or(&um.path);
            let title_max = w.saturating_sub(4);

            Line::from(vec![
                Span::styled("    ", Style::default()), // align with checkbox area
                Span::styled(
                    crate::helpers::truncate_right(filename, title_max),
                    label_style,
                ),
            ])
        }
    }
}

// ============================================================================
// Wizard Content Builders (called from mod.rs rebuild_entries)
// ============================================================================

/// Build release overview lines for the wizard popup (z key).
pub fn build_release_overview_lines(
    release: &ReleaseGroup,
    category: PackingCategory,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Low confidence warning banner
    if let Some(ref reason) = release.low_confidence_reason {
        lines.push(Line::from(Span::styled(
            "\u{26a0} LOW CONFIDENCE \u{2014} PROBABLE MISPACK \u{26a0}",
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

    // File type breakdown
    let mut ext_counts: HashMap<String, usize> = HashMap::new();
    let mut dir_counts: HashMap<String, usize> = HashMap::new();

    for track in &release.tracks {
        let ext = track
            .path
            .rsplit('.')
            .next()
            .unwrap_or("?")
            .to_lowercase();
        *ext_counts.entry(ext).or_default() += 1;

        let dir = match track.path.rsplit_once('/') {
            Some((parent, _)) => parent.to_string(),
            None => "?".to_string(),
        };
        *dir_counts.entry(dir).or_default() += 1;
    }

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
        "\u{2500}\u{2500} Source Directories \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
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
            "\u{2500}\u{2500} VA Override \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
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
                "\u{2500}\u{2500} Alternatives ({}) \u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}",
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

/// Build interleaved track list + score breakdown cards for the wizard pane (Z key).
pub fn build_track_pane_content(
    release: &ReleaseGroup,
    _category: PackingCategory,
) -> Vec<RichBlock> {
    let mut blocks = Vec::new();

    // Unfilled slots at the top
    if !release.unfilled.is_empty() {
        blocks.push(RichBlock::Heading("Unfilled Slots".to_string()));
        for slot in &release.unfilled {
            blocks.push(RichBlock::Paragraph(vec![
                RichSpan::new(
                    format!("  {:>2}.{:>2}  ", slot.medium_pos, slot.track_pos),
                    Style::default().fg(Color::DarkGray),
                ),
                RichSpan::new("\u{2591} ", Style::default().fg(Color::Red)),
                RichSpan::new(
                    slot.track_title.clone(),
                    Style::default()
                        .fg(Color::DarkGray)
                        .add_modifier(Modifier::ITALIC),
                ),
            ]));
        }
        blocks.push(RichBlock::Separator);
    }

    // Assigned tracks with score breakdown cards
    blocks.push(RichBlock::Heading("Assigned Tracks".to_string()));
    blocks.push(RichBlock::Blank);

    for track in &release.tracks {
        let filename = track.path.rsplit('/').next().unwrap_or(&track.path);

        // Track row: [medium.track] Title - filename  score
        blocks.push(RichBlock::Paragraph(vec![
            RichSpan::new(
                format!("{:>2}.{:>2}  ", track.medium_position, track.track_position),
                Style::default().fg(Color::DarkGray),
            ),
            RichSpan::new(
                track.track_title.clone(),
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ]));
        blocks.push(RichBlock::Paragraph(vec![
            RichSpan::new(
                format!("        {}", filename),
                Style::default().fg(Color::DarkGray),
            ),
            RichSpan::new(
                format!("  score: {:.3}", track.score),
                Style::default().fg(Color::Yellow),
            ),
        ]));

        // Score breakdown table
        let breakdown = &track.score_breakdown;
        let scores = [
            ("AcoustID", breakdown.acoustid_confidence),
            ("Duration", breakdown.duration_match),
            ("Title", breakdown.title_match),
            ("Artist", breakdown.artist_match),
            ("Album", breakdown.album_match),
            ("Track #", breakdown.track_number_match),
        ];

        let headers = vec![
            RichSpan::new("Component", Style::default().fg(Color::DarkGray)),
            RichSpan::new("Score", Style::default().fg(Color::DarkGray)),
            RichSpan::new("Bar", Style::default().fg(Color::DarkGray)),
        ];

        let rows: Vec<Vec<Vec<RichSpan>>> = scores
            .iter()
            .map(|(label, value)| {
                let bar = build_score_bar(*value);
                vec![
                    vec![RichSpan::new(
                        format!("  {}", label),
                        Style::default().fg(Color::DarkGray),
                    )],
                    vec![RichSpan::new(
                        format!("{:.2}", value),
                        Style::default().fg(score_color(*value)),
                    )],
                    vec![RichSpan::new(bar, Style::default().fg(score_color(*value)))],
                ]
            })
            .collect();

        blocks.push(RichBlock::Table {
            headers,
            rows,
            col_ratio: vec![30, 20, 50],
        });

        // Extra info: alternatives, recording ID
        blocks.push(RichBlock::Paragraph(vec![
            RichSpan::new("        Rec: ", Style::default().fg(Color::DarkGray)),
            RichSpan::new(
                track.recording_id.clone(),
                Style::default().fg(Color::DarkGray),
            ),
            RichSpan::new(
                format!("  ({} alt)", track.alternatives_count),
                Style::default().fg(Color::DarkGray),
            ),
        ]));

        blocks.push(RichBlock::Blank);
    }

    blocks
}

/// Build a compact score bar string.
fn build_score_bar(value: f64) -> String {
    const BAR_WIDTH: usize = 12;
    let filled = ((value * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    format!(
        "{}{}",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(empty)
    )
}

/// Color for a score value.
fn score_color(value: f64) -> Color {
    if value >= 0.8 {
        Color::Green
    } else if value >= 0.5 {
        Color::Yellow
    } else {
        Color::Red
    }
}

// ============================================================================
// Pin Release Overlay
// ============================================================================

fn render_pin_overlay(f: &mut Frame, area: Rect, state: &ReleasePackingBrowserState) {
    let input = match &state.pin_input {
        Some(input) => input,
        None => return,
    };

    // Height: border + uuid_line + (error|spacer) + hints + border = 5
    let height = 5u16;
    let width = 46u16.min(area.width.saturating_sub(4));
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let popup_area = Rect::new(x, y, width, height);

    // Clear one extra cell around the popup for visual separation
    let padded_area = Rect::new(
        popup_area.x.saturating_sub(1),
        popup_area.y.saturating_sub(1),
        (popup_area.width + 2).min(area.x + area.width - popup_area.x.saturating_sub(1)),
        (popup_area.height + 2).min(area.y + area.height - popup_area.y.saturating_sub(1)),
    );
    f.render_widget(Clear, padded_area);

    let block = Block::default()
        .title(" Pin MusicBrainz Release ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::LightBlue));
    let inner = block.inner(popup_area);
    f.render_widget(block, popup_area);

    let mut lines: Vec<Line> = Vec::new();

    // UUID template: ________-____-____-____-____________ with cursor
    lines.push(render_uuid_template(
        input.value(),
        input.cursor,
        inner.width,
    ));

    // Error replaces the spacer line
    if let Some(ref err) = state.pin_error {
        lines.push(
            Line::from(Span::styled(err.clone(), Style::default().fg(Color::Red))).centered(),
        );
    } else {
        lines.push(Line::from(Span::raw("")));
    }

    // Centered hints
    lines.push(
        Line::from(vec![
            control_colors::nav("Enter"),
            control_colors::text(" confirm  "),
            control_colors::cancel("Esc"),
            control_colors::text(" cancel"),
        ])
        .centered(),
    );

    f.render_widget(Paragraph::new(lines), inner);
}

/// Render the UUID hex value as a `________-____-____-____-____________` template.
fn render_uuid_template(hex: &str, cursor: usize, inner_width: u16) -> Line<'static> {
    const DASH_POSITIONS: [usize; 4] = [8, 13, 18, 23];
    let hex_chars: Vec<char> = hex.chars().collect();
    let cursor_display = hex_cursor_to_display(cursor);

    let mut spans: Vec<Span> = Vec::new();

    // Center the 36-char UUID template within the inner width
    let uuid_width = 36usize;
    let left_pad = (inner_width as usize).saturating_sub(uuid_width) / 2;
    if left_pad > 0 {
        spans.push(Span::raw(" ".repeat(left_pad)));
    }

    let mut hex_idx = 0;
    for display_idx in 0..36usize {
        let is_cursor = display_idx == cursor_display;

        if DASH_POSITIONS.contains(&display_idx) {
            let style = if is_cursor {
                Style::default().fg(Color::Black).bg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            spans.push(Span::styled("-", style));
        } else {
            if hex_idx < hex_chars.len() {
                let style = if is_cursor {
                    Style::default().fg(Color::Black).bg(Color::White)
                } else {
                    Style::default().fg(Color::White)
                };
                spans.push(Span::styled(hex_chars[hex_idx].to_string(), style));
            } else {
                let style = if is_cursor {
                    Style::default().fg(Color::Black).bg(Color::White)
                } else {
                    Style::default().fg(Color::DarkGray)
                };
                spans.push(Span::styled("_", style));
            }
            hex_idx += 1;
        }
    }

    Line::from(spans)
}

/// Map a hex-only cursor position (0..32) to its display position (0..36)
/// accounting for dash positions at display indices 8, 13, 18, 23.
fn hex_cursor_to_display(hex_pos: usize) -> usize {
    if hex_pos >= 20 {
        hex_pos + 4
    } else if hex_pos >= 16 {
        hex_pos + 3
    } else if hex_pos >= 12 {
        hex_pos + 2
    } else if hex_pos >= 8 {
        hex_pos + 1
    } else {
        hex_pos
    }
}

// ============================================================================
// Helpers
// ============================================================================

use crate::helpers::kv_line;

/// Coverage color: yellow for LowConfidence (never green), normal thresholds otherwise.
fn coverage_color(release: &ReleaseGroup, category: PackingCategory) -> Color {
    if category == PackingCategory::LowConfidence {
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
