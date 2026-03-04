//! Rendering for the External Matches lateral view.
//!
//! Two-pane layout (65/35):
//! - Left: Navigable entry list (fetch action + confidence-bucketed entries)
//! - Right: Context-sensitive detail pane

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::{ExternalMatchesViewState, NavigableEntry};
use crate::meta::views::ConfidenceTier;
// Hint text color for "Press Enter to..." prompts
const HINT_COLOR: Color = Color::DarkGray;

pub fn render(f: &mut Frame, area: Rect, state: &mut ExternalMatchesViewState) {
    // Two-pane horizontal split: 65% left, 35% right
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(65),
            Constraint::Percentage(35),
        ])
        .split(area);

    render_left_pane(f, chunks[0], state);
    render_right_pane(f, chunks[1], state);
}

// ============================================================================
// Left Pane: Entry List
// ============================================================================

fn render_left_pane(f: &mut Frame, area: Rect, state: &mut ExternalMatchesViewState) {
    let block = Block::default()
        .title(" Ext. Matches ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    let mut lines: Vec<Line> = Vec::new();

    // Populate click targets: track line → nav_index mapping
    state.click_targets.clear();
    state.click_targets.set_list_area(inner);

    // Section header: Actions
    lines.push(Line::from(Span::styled(
        "── Actions ──────────────",
        Style::default().fg(Color::DarkGray),
    )));

    // Track which navigable entry index we're on
    let mut nav_index = 0;

    // Fetch entry (always first navigable entry)
    {
        let selected = state.cursor == nav_index;
        let (status_label, status_color) = if !state.has_api_key {
            ("No API Key".to_string(), Color::Red)
        } else if state.fetch_active {
            if let Some(ref p) = state.fetch_progress {
                let a = &p.acoustid;
                let m = &p.mb;
                if m.total > 0 && a.total > 0 {
                    (format!("{}/{} + MB {}/{}", a.processed, a.total, m.processed, m.total), Color::Yellow)
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

        let marker = if selected { "▸ " } else { "  " };
        let label_style = if selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if !state.has_api_key || state.fetch_active {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };

        let line_idx = lines.len();
        lines.push(Line::from(vec![
            Span::styled(marker, label_style),
            Span::styled("Cache external metadata matches  ", label_style),
            Span::styled(format!("{:<12}", status_label), Style::default().fg(status_color)),
        ]));
        state.click_targets.add_row(nav_index.to_string(), inner.y + line_idx as u16);
        nav_index += 1;
    }

    // Pack releases entry
    {
        let selected = state.cursor == nav_index;
        let has_data = state.cached_data.as_ref().is_some_and(|d| {
            !d.untagged_entries.is_empty() || !d.confidence_buckets.is_empty()
        });
        let (status_label, status_color) = if state.fetch_active {
            ("Fetch active", Color::DarkGray)
        } else if !has_data {
            ("No data", Color::DarkGray)
        } else {
            ("Ready", Color::Green)
        };

        let marker = if selected { "▸ " } else { "  " };
        let label_style = if selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if state.fetch_active || !has_data {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };

        let line_idx = lines.len();
        lines.push(Line::from(vec![
            Span::styled(marker, label_style),
            Span::styled("Analyze release matches   ", label_style),
            Span::styled(format!("{:<12}", status_label), Style::default().fg(status_color)),
        ]));
        state.click_targets.add_row(nav_index.to_string(), inner.y + line_idx as u16);
        nav_index += 1;
    }

    // Section header: Matches (only if there's data)
    if let Some(ref data) = state.cached_data {
        let has_any = !data.untagged_entries.is_empty() || !data.confidence_buckets.is_empty();
        if has_any {
            lines.push(Line::from(Span::raw(""))); // spacer
            lines.push(Line::from(Span::styled(
                "── Matches ──────────────",
                Style::default().fg(Color::DarkGray),
            )));

            // Untagged matches entry (before confidence tiers)
            if !data.untagged_entries.is_empty() {
                let selected = state.cursor == nav_index;
                let line_idx = lines.len();
                lines.push(render_bucket_line_styled(
                    selected,
                    "?",
                    Color::Magenta,
                    "Untagged matches",
                    data.untagged_entries.len(),
                    false,
                ));
                state.click_targets.add_row(nav_index.to_string(), inner.y + line_idx as u16);
                nav_index += 1;
            }

            // Confidence tier entries
            for bucket in &data.confidence_buckets {
                let selected = state.cursor == nav_index;
                let (marker_color, label_dim) = match bucket.tier {
                    ConfidenceTier::Perfect | ConfidenceTier::VeryHigh | ConfidenceTier::High => {
                        (Color::Yellow, false)
                    }
                    ConfidenceTier::Medium => (Color::Yellow, true),
                    ConfidenceTier::Low => (Color::DarkGray, false),
                };

                let label = format!("{} confidence", bucket.tier.label());
                let line_idx = lines.len();
                lines.push(render_bucket_line_styled(
                    selected,
                    "!",
                    marker_color,
                    &label,
                    bucket.total,
                    label_dim,
                ));
                state.click_targets.add_row(nav_index.to_string(), inner.y + line_idx as u16);
                nav_index += 1;
            }
            // Release Packing section (after confidence tiers)
            if data.packing_assigned_count > 0 || data.packing_unmatched_count > 0 {
                lines.push(Line::from(Span::raw(""))); // spacer
                lines.push(Line::from(Span::styled(
                    "── Release Packing ──────",
                    Style::default().fg(Color::DarkGray),
                )));

                let selected = state.cursor == nav_index;
                let marker = if selected { "▸ " } else { "  " };
                let label_style = if selected {
                    Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };

                // Summary subtitle
                let mut parts = Vec::new();
                if data.packing_release_count > 0 {
                    parts.push(format!("{} releases", data.packing_release_count));
                }
                if data.packing_unfilled_count > 0 {
                    parts.push(format!("{} unfilled", data.packing_unfilled_count));
                }
                if data.packing_near_miss_count > 0 {
                    parts.push(format!("{} near-miss", data.packing_near_miss_count));
                }

                let line_idx = lines.len();
                lines.push(Line::from(vec![
                    Span::styled(marker.to_string(), label_style),
                    Span::styled("R".to_string(), Style::default().fg(Color::Cyan)),
                    Span::styled(format!(" {:<24}", "Release assignments"), label_style),
                    Span::styled(format!("{:>6}", data.packing_assigned_count), Style::default().fg(Color::Yellow)),
                ]));
                state.click_targets.add_row(nav_index.to_string(), inner.y + line_idx as u16);

                // Subtitle line (indented, non-navigable)
                if !parts.is_empty() {
                    lines.push(Line::from(Span::styled(
                        format!("      {}", parts.join(", ")),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
                let _ = nav_index; // last entry, suppress unused warning
            }
        }
    } else {
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "  Loading...",
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Apply scroll
    let visible_height = inner.height as usize;
    let total_lines = lines.len();
    // Simple scroll: ensure cursor-related line is visible
    // Each navigable entry maps roughly to a line, but section headers shift things.
    // For simplicity, just render from top — the list is typically short.
    let skip = if total_lines > visible_height {
        total_lines.saturating_sub(visible_height).min(state.scroll)
    } else {
        0
    };

    let visible_lines: Vec<Line> = lines.into_iter().skip(skip).take(visible_height).collect();
    let paragraph = Paragraph::new(visible_lines);
    f.render_widget(paragraph, inner);
}

fn render_bucket_line_styled(
    selected: bool,
    marker_char: &str,
    marker_color: Color,
    label: &str,
    count: usize,
    dim: bool,
) -> Line<'static> {
    let cursor_marker = if selected { "▸ " } else { "  " };
    let label_style = if selected {
        Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
    } else if dim {
        Style::default().fg(Color::DarkGray)
    } else {
        Style::default().fg(Color::White)
    };

    Line::from(vec![
        Span::styled(cursor_marker.to_string(), label_style),
        Span::styled(marker_char.to_string(), Style::default().fg(marker_color)),
        Span::styled(format!(" {:<24}", label), label_style),
        Span::styled(format!("{:>6}", count), Style::default().fg(Color::Yellow)),
    ])
}

// ============================================================================
// Right Pane: Detail
// ============================================================================

fn render_right_pane(f: &mut Frame, area: Rect, state: &ExternalMatchesViewState) {
    let block = Block::default()
        .title(" Details ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let inner = block.inner(area);
    f.render_widget(block, area);

    if inner.height < 2 || inner.width < 10 {
        return;
    }

    let selected = state.selected_entry();
    let lines = match selected {
        Some(NavigableEntry::FetchAction) => render_fetch_detail(state),
        Some(NavigableEntry::PackReleasesAction) => render_pack_releases_detail(state),
        Some(NavigableEntry::UntaggedMatches) => render_untagged_detail(state),
        Some(NavigableEntry::ConfidenceBucket(tier)) => render_tier_detail(state, tier),
        Some(NavigableEntry::ReleasePackingResults) => render_packing_results_detail(state),
        None => vec![Line::from(Span::styled(
            "No selection",
            Style::default().fg(Color::DarkGray),
        ))],
    };

    let paragraph = Paragraph::new(lines);
    f.render_widget(paragraph, inner);
}

fn render_fetch_detail(state: &ExternalMatchesViewState) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Cache external metadata matches",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
    ];

    if !state.has_api_key {
        // No API key configured
        lines.push(Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
            Span::styled("No API Key", Style::default().fg(Color::Red)),
        ]));
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "Configure an AcoustID API key",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "in Config to enable lookups.",
            Style::default().fg(Color::DarkGray),
        )));
    } else if state.fetch_active {
        // Active batch with progress
        if let Some(ref p) = state.fetch_progress {
            let a = &p.acoustid;
            let m = &p.mb;

            // AcoustID section
            if a.total > 0 {
                lines.push(Line::from(vec![
                    Span::styled("AcoustID:  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{}/{}", a.processed, a.total),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Matched:    ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{:>5}", a.matched), Style::default().fg(Color::Green)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  No match:   ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{:>5}", a.no_match), Style::default().fg(Color::White)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Retries:    ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{:>5}", a.retries), Style::default().fg(Color::Yellow)),
                ]));
            }

            // MB section (only shown when MB has work)
            if m.total > 0 {
                if a.total > 0 { lines.push(Line::from(Span::raw(""))); }
                lines.push(Line::from(vec![
                    Span::styled("MusicBrainz: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{}/{}", m.processed, m.total),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Good fetch: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{:>5}", m.matched), Style::default().fg(Color::Green)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Not found:  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{:>5}", m.no_match), Style::default().fg(Color::White)),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Retries:    ", Style::default().fg(Color::DarkGray)),
                    Span::styled(format!("{:>5}", m.retries), Style::default().fg(Color::Yellow)),
                ]));
            }

            lines.push(Line::from(Span::raw("")));

            // Progress bar: show combined progress
            let total_processed = a.processed + m.processed;
            let total_items = a.total + m.total;
            lines.push(Line::from(render_braille_bar(total_processed, total_items, state.tick_count)));

            // ETA: use live rates from the scheduler's rate limiters
            let a_remaining = a.total.saturating_sub(a.processed);
            let m_remaining = m.total.saturating_sub(m.processed);
            let a_rps = p.acoustid_rps.max(0.1);
            let m_rps = p.mb_rps.max(0.1);
            let secs = (a_remaining as f32 / a_rps + m_remaining as f32 / m_rps) as u64;
            if secs > 0 {
                let eta = if secs >= 3600 {
                    format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
                } else if secs >= 60 {
                    format!("{}m {:02}s", secs / 60, secs % 60)
                } else {
                    format!("{}s", secs)
                };
                lines.push(Line::from(Span::styled(
                    format!("  ETA: ~{}", eta),
                    Style::default().fg(Color::DarkGray),
                )));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled("Active", Style::default().fg(Color::Yellow)),
            ]));
        }
    } else {
        // Idle — show last-batch summary if available
        lines.push(Line::from(vec![
            Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
            Span::styled("Idle", Style::default().fg(Color::Green)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("API Key: ", Style::default().fg(Color::DarkGray)),
            Span::styled("configured", Style::default().fg(Color::Green)),
        ]));

        if let Some(ref p) = state.fetch_progress {
            let a = &p.acoustid;
            let m = &p.mb;
            let total = a.total + m.total;
            if total > 0 {
                lines.push(Line::from(Span::raw("")));
                if a.total > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("Last AcoustID: {} processed", a.total),
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(vec![
                        Span::styled("  Matched: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("{}", a.matched), Style::default().fg(Color::Green)),
                        Span::styled("  No match: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(format!("{}", a.no_match), Style::default().fg(Color::White)),
                    ]));
                }
                if m.total > 0 {
                    lines.push(Line::from(Span::styled(
                        format!("Last MB: {} good fetch", m.matched),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }

        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "Press Enter to start lookup.",
            Style::default().fg(HINT_COLOR),
        )));
    }

    lines
}

/// Render a braille progress bar as a vector of Spans.
///
/// Uses the same braille style as the startup progress screen:
/// ⠸ (left bracket), ⠿ (filled), animated spinner, spaces (empty), ⠇ (right bracket).
fn render_braille_bar(processed: usize, total: usize, tick_count: u32) -> Vec<Span<'static>> {
    const BAR_WIDTH: usize = 20;
    // Bouncing dot animation — same pattern as progress_screen.rs
    const BOUNCE_LEFT: &[char] = &['⠁', '⠂', '⠄', '⠂'];
    const BOUNCE_RIGHT: &[char] = &['⠏', '⠗', '⠧', '⠗'];

    if total == 0 {
        return vec![Span::styled(
            format!("  ⠸{}⠇", " ".repeat(BAR_WIDTH)),
            Style::default().fg(Color::DarkGray),
        )];
    }

    let ratio = (processed as f32 / total as f32).min(1.0);
    let pct = (ratio * 100.0).round() as u32;

    // Half-cell granularity: each cell has a left and right column
    let half_cells = ((ratio * (BAR_WIDTH * 2) as f32) as usize).min(BAR_WIDTH * 2);
    let full_cells = half_cells / 2;
    let has_half = half_cells % 2 == 1;

    let spinner = if processed >= total {
        '⠿'
    } else {
        let anim_idx = ((tick_count / 2) as usize) % BOUNCE_LEFT.len();
        if has_half {
            BOUNCE_RIGHT[anim_idx]
        } else {
            BOUNCE_LEFT[anim_idx]
        }
    };

    let spinner_cells = if processed < total { 1 } else { 0 };
    let empty = BAR_WIDTH.saturating_sub(full_cells + spinner_cells);

    let bar = format!(
        "  ⠸{}{}{}⠇  {}%",
        "⠿".repeat(full_cells),
        spinner,
        " ".repeat(empty),
        pct,
    );

    let bar_color = if processed >= total {
        Color::Green
    } else {
        Color::Rgb(241, 92, 153) // #f15c99 — same accent as startup bar
    };

    vec![Span::styled(bar, Style::default().fg(bar_color))]
}

fn render_pack_releases_detail(state: &ExternalMatchesViewState) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Analyze release matches",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
    ];

    let has_data = state.cached_data.as_ref().is_some_and(|d| {
        !d.confidence_buckets.is_empty()
    });

    if state.fetch_active {
        lines.push(Line::from(Span::styled(
            "Wait for the external fetch to",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "complete before running analysis.",
            Style::default().fg(Color::DarkGray),
        )));
    } else if !has_data {
        lines.push(Line::from(Span::styled(
            "No external match data available.",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "Run a fetch first to populate",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "recording and release data.",
            Style::default().fg(Color::DarkGray),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "Bin-pack recordings into releases",
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(Span::styled(
            "using cached MusicBrainz data.",
            Style::default().fg(Color::White),
        )));
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "Scores each file by AcoustID",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "confidence, duration match, tag",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "similarity, and directory cohesion.",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::raw("")));
        lines.push(Line::from(Span::styled(
            "Press Enter to start analysis.",
            Style::default().fg(HINT_COLOR),
        )));
    }

    lines
}

fn render_untagged_detail(state: &ExternalMatchesViewState) -> Vec<Line<'static>> {
    let count = state.cached_data.as_ref()
        .map(|d| d.untagged_entries.len())
        .unwrap_or(0);

    vec![
        Line::from(Span::styled(
            "Untagged Matches",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            format!("{} files with fingerprint matches", count),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "but no existing tags for matched fields.",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Press Enter to browse.",
            Style::default().fg(HINT_COLOR),
        )),
    ]
}

fn render_packing_results_detail(state: &ExternalMatchesViewState) -> Vec<Line<'static>> {
    let mut lines = vec![
        Line::from(Span::styled(
            "Release Packing Results",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
    ];

    if let Some(ref data) = state.cached_data {
        lines.push(Line::from(vec![
            Span::styled("Assigned:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", data.packing_assigned_count), Style::default().fg(Color::Green)),
            Span::styled(" tracks", Style::default().fg(Color::DarkGray)),
        ]));
        lines.push(Line::from(vec![
            Span::styled("Releases:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(format!("{}", data.packing_release_count), Style::default().fg(Color::Yellow)),
        ]));
        if data.packing_unfilled_count > 0 {
            lines.push(Line::from(vec![
                Span::styled("Unfilled:   ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}", data.packing_unfilled_count), Style::default().fg(Color::Red)),
                Span::styled(" slots", Style::default().fg(Color::DarkGray)),
            ]));
        }
        if data.packing_near_miss_count > 0 {
            lines.push(Line::from(vec![
                Span::styled("Near-miss:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}", data.packing_near_miss_count), Style::default().fg(Color::Yellow)),
            ]));
        }
        if data.packing_unmatched_count > 0 {
            lines.push(Line::from(vec![
                Span::styled("Unmatched:  ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}", data.packing_unmatched_count), Style::default().fg(Color::DarkGray)),
                Span::styled(" tracks", Style::default().fg(Color::DarkGray)),
            ]));
        }
    }

    lines.push(Line::from(Span::raw("")));
    lines.push(Line::from(Span::styled(
        "Press Enter to browse.",
        Style::default().fg(HINT_COLOR),
    )));

    lines
}

fn render_tier_detail(state: &ExternalMatchesViewState, tier: ConfidenceTier) -> Vec<Line<'static>> {
    let total = state.cached_data.as_ref()
        .and_then(|d| d.confidence_buckets.iter().find(|b| b.tier == tier))
        .map(|b| b.total)
        .unwrap_or(0);

    let title = format!("{} Confidence Matches", tier.label());

    let description = match tier {
        ConfidenceTier::Perfect => format!("{} files with fingerprint confidence = 100%.", total),
        ConfidenceTier::VeryHigh => format!("{} files with fingerprint confidence 99%+.", total),
        ConfidenceTier::High => format!("{} files with fingerprint confidence 95%+.", total),
        ConfidenceTier::Medium => format!("{} files with fingerprint confidence 90%+.", total),
        ConfidenceTier::Low => format!("{} files with fingerprint confidence < 90%.", total),
    };

    let mut lines = vec![
        Line::from(Span::styled(
            title,
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            description,
            Style::default().fg(Color::White),
        )),
        Line::from(Span::raw("")),
    ];

    let hint = match tier {
        ConfidenceTier::Perfect | ConfidenceTier::VeryHigh => "Highest impact matches. Press Enter to browse.",
        ConfidenceTier::High => "High confidence matches. Press Enter to browse.",
        ConfidenceTier::Medium => "Medium confidence. Press Enter to browse.",
        ConfidenceTier::Low => "Low confidence. May contain false positives.",
    };

    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(HINT_COLOR),
    )));

    lines
}
