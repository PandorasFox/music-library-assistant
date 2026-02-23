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

pub fn render(f: &mut Frame, area: Rect, state: &ExternalMatchesViewState) {
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

fn render_left_pane(f: &mut Frame, area: Rect, state: &ExternalMatchesViewState) {
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
            ("No API Key", Color::Red)
        } else if state.fetch_active {
            ("Active", Color::Yellow)
        } else {
            ("Idle", Color::Green)
        };

        let marker = if selected { "▸ " } else { "  " };
        let label_style = if selected {
            Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)
        } else if !state.has_api_key || state.fetch_active {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default().fg(Color::White)
        };

        lines.push(Line::from(vec![
            Span::styled(marker, label_style),
            Span::styled("Fetch AcoustID Data  ", label_style),
            Span::styled(status_label, Style::default().fg(status_color)),
        ]));
        nav_index += 1;
    }

    // Section header: Matches (only if there's data)
    if let Some(ref data) = state.cached_data {
        let has_entries = !data.untagged_entries.is_empty() || !data.confidence_buckets.is_empty();
        if has_entries {
            lines.push(Line::from(Span::raw(""))); // spacer
            lines.push(Line::from(Span::styled(
                "── Matches ──────────────",
                Style::default().fg(Color::DarkGray),
            )));

            // Untagged files entry
            if !data.untagged_entries.is_empty() {
                let count = data.untagged_entries.len();
                let selected = state.cursor == nav_index;
                let marker_char = "?";
                let marker_color = Color::Cyan;
                lines.push(render_bucket_line(
                    selected,
                    marker_char,
                    marker_color,
                    "Untagged files",
                    count,
                ));
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
                lines.push(render_bucket_line_styled(
                    selected,
                    "!",
                    marker_color,
                    &label,
                    bucket.total,
                    label_dim,
                ));
                nav_index += 1;
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

fn render_bucket_line(
    selected: bool,
    marker_char: &str,
    marker_color: Color,
    label: &str,
    count: usize,
) -> Line<'static> {
    render_bucket_line_styled(selected, marker_char, marker_color, label, count, false)
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
        Some(NavigableEntry::UntaggedBucket) => render_untagged_detail(state),
        Some(NavigableEntry::ConfidenceBucket(tier)) => render_tier_detail(state, tier),
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
            "AcoustID Fetch",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
    ];

    // Status
    let (status_text, status_color) = if !state.has_api_key {
        ("No API Key", Color::Red)
    } else if state.fetch_active {
        ("Active", Color::Yellow)
    } else {
        ("Idle", Color::Green)
    };

    lines.push(Line::from(vec![
        Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
        Span::styled(status_text, Style::default().fg(status_color)),
    ]));

    // API key status
    let key_text = if state.has_api_key { "configured" } else { "not configured" };
    let key_color = if state.has_api_key { Color::Green } else { Color::Red };
    lines.push(Line::from(vec![
        Span::styled("API Key: ", Style::default().fg(Color::DarkGray)),
        Span::styled(key_text, Style::default().fg(key_color)),
    ]));

    lines.push(Line::from(Span::raw("")));

    if !state.has_api_key {
        lines.push(Line::from(Span::styled(
            "Configure an AcoustID API key",
            Style::default().fg(Color::DarkGray),
        )));
        lines.push(Line::from(Span::styled(
            "in Config to enable lookups.",
            Style::default().fg(Color::DarkGray),
        )));
    } else if state.fetch_active {
        lines.push(Line::from(Span::styled(
            "Batch in progress...",
            Style::default().fg(Color::Yellow),
        )));
    } else {
        lines.push(Line::from(Span::styled(
            "Press Enter to start lookup.",
            Style::default().fg(HINT_COLOR),
        )));
    }

    lines
}

fn render_untagged_detail(state: &ExternalMatchesViewState) -> Vec<Line<'static>> {
    let count = state.cached_data.as_ref()
        .map(|d| d.untagged_entries.len())
        .unwrap_or(0);

    let lines = vec![
        Line::from(Span::styled(
            "Untagged Files",
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            format!("{} files have no relevant tags in corpus", count),
            Style::default().fg(Color::White),
        )),
        Line::from(Span::styled(
            "but AcoustID found recording metadata.",
            Style::default().fg(Color::White),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Accepting these will populate tags from",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::styled(
            "external sources (title, artist, album).",
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(Span::raw("")),
        Line::from(Span::styled(
            "Press Enter to review and import.",
            Style::default().fg(HINT_COLOR),
        )),
    ];

    lines
}

fn render_tier_detail(state: &ExternalMatchesViewState, tier: ConfidenceTier) -> Vec<Line<'static>> {
    let bucket = state.cached_data.as_ref()
        .and_then(|d| d.confidence_buckets.iter().find(|b| b.tier == tier));

    let (total, content_diff, metadata_only) = match bucket {
        Some(b) => (b.total, b.content_diff_count, b.metadata_only_count),
        None => (0, 0, 0),
    };

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

    // ContentDiff / MetadataOnly breakdown
    lines.push(Line::from(vec![
        Span::styled("  Content diffs:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:>4}", content_diff),
            Style::default().fg(Color::Yellow),
        ),
        Span::styled("  (tag values differ)", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::styled("  Metadata only:  ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:>4}", metadata_only),
            Style::default().fg(Color::Cyan),
        ),
        Span::styled("  (corpus missing tags)", Style::default().fg(Color::DarkGray)),
    ]));

    lines.push(Line::from(Span::raw("")));

    let hint = match tier {
        ConfidenceTier::Perfect | ConfidenceTier::VeryHigh => "Highest impact matches. Press Enter to review.",
        ConfidenceTier::High => "High confidence matches. Press Enter to review.",
        ConfidenceTier::Medium => "Medium confidence. Review with care.",
        ConfidenceTier::Low => "Low confidence. May contain false positives.",
    };

    lines.push(Line::from(Span::styled(
        hint,
        Style::default().fg(HINT_COLOR),
    )));

    lines
}
