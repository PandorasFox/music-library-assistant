//! Shared UI rendering utilities
//!
//! Pure logic re-exported from mm-ui. Rendering helpers stay here.

pub use mm_ui::helpers::{
    clamp_scroll, format_bytes, format_duration_ms, format_kbps, format_sample_rate, format_si,
    handle_scroll_input, truncate_left, truncate_right, StashCancelButton,
};

use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear};
use ratatui::Frame;

// ============================================================================
// Pane Rendering Utilities
// ============================================================================

/// Render a block and return its inner area, properly cleared.
///
/// This is the standard way to render panes in MM. It:
/// 1. Computes the inner area from the block
/// 2. Renders the block (borders, title, etc.)
/// 3. Clears the inner area to prevent leftover content from showing through
/// 4. Returns the inner Rect for content rendering
///
/// Without clearing, list widgets that don't fill their area will show
/// "ghost" content from previous renders.
pub fn render_pane(f: &mut Frame, area: Rect, block: Block) -> Rect {
    let inner = block.inner(area);
    f.render_widget(block, area);
    f.render_widget(Clear, inner);
    inner
}

/// Create a bordered block with focus-aware border color.
///
/// Yellow border when focused, DarkGray when not. This is the standard
/// pane border pattern used across all multi-pane views.
pub fn focused_block(title: &str, is_focused: bool) -> Block<'_> {
    let border_color = if is_focused {
        Color::Yellow
    } else {
        Color::DarkGray
    };
    Block::default()
        .title(title)
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
}

// ============================================================================
// Rendering Helpers
// ============================================================================

/// Render a label-value line with DarkGray label (padded to 11 chars) and White value.
pub fn kv_line(label: &str, value: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(
            format!("{:<11}", label),
            Style::default().fg(Color::DarkGray),
        ),
        Span::styled(value.to_string(), Style::default().fg(Color::White)),
    ])
}

/// Render a score bar: label + numeric value + filled/empty block bar.
pub fn render_score_bar(label: &str, value: f64) -> Line<'static> {
    const BAR_WIDTH: usize = 20;
    let filled = ((value * BAR_WIDTH as f64).round() as usize).min(BAR_WIDTH);
    let empty = BAR_WIDTH - filled;
    let bar = format!("{}{}", "\u{2588}".repeat(filled), " ".repeat(empty));

    Line::from(vec![
        Span::styled(format!("  {}", label), Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!(" {:.2}  ", value),
            Style::default().fg(Color::White),
        ),
        Span::styled(bar, Style::default().fg(Color::Yellow)),
    ])
}

/// Render audio metadata lines (Format/Duration/Bitrate/SampleRate/Size/AlbumArt)
/// from a `FileMetaSummary`. Returns lines to extend into caller's buffer.
pub fn render_audio_metadata_lines(
    meta: &crate::manual_review_modal::types::FileMetaSummary,
    label_style: Style,
    value_style: Style,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    lines.push(Line::from(""));

    lines.push(Line::from(vec![
        Span::styled("Format: ", label_style),
        Span::styled(meta.file_type.to_uppercase(), value_style),
    ]));

    if let Some(dur) = meta.duration_ms {
        lines.push(Line::from(vec![
            Span::styled("Duration: ", label_style),
            Span::styled(format_duration_ms(dur), value_style),
        ]));
    }

    if let Some(br) = meta.bitrate_kbps {
        lines.push(Line::from(vec![
            Span::styled("Bitrate: ", label_style),
            Span::styled(format_kbps(br), value_style),
        ]));
    }

    if let Some(sr) = meta.sample_rate {
        lines.push(Line::from(vec![
            Span::styled("Sample rate: ", label_style),
            Span::styled(format_sample_rate(sr), value_style),
        ]));
    }

    if meta.file_size > 0 {
        lines.push(Line::from(vec![
            Span::styled("Size: ", label_style),
            Span::styled(format_bytes(meta.file_size as u64), value_style),
        ]));
    }

    let art_label = if meta.has_pictures { "Yes" } else { "No" };
    let art_style = if meta.has_pictures {
        Style::default().fg(Color::Green)
    } else {
        Style::default().fg(Color::DarkGray)
    };
    lines.push(Line::from(vec![
        Span::styled("Album art: ", label_style),
        Span::styled(art_label, art_style),
    ]));

    lines
}

/// Render "── Pending edits ──" separator and change lines for staged tag edits.
///
/// Returns lines to append. Caller decides how to wrap them (ListItem, Paragraph, etc.).
pub fn render_pending_edit_lines(
    edits: &[(String, String, String)],
    max_width: usize,
    remaining_height: usize,
) -> Vec<Line<'static>> {
    let mut lines = Vec::new();
    if edits.is_empty() || remaining_height == 0 {
        return lines;
    }

    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "\u{2500}\u{2500} Pending edits \u{2500}\u{2500}",
        Style::default().fg(Color::Magenta),
    )));
    for (tag, old, new) in edits {
        if lines.len() >= remaining_height {
            break;
        }
        let change = if old.is_empty() {
            format!("{}: +\"{}\"", tag, new)
        } else if new.is_empty() {
            format!("{}: -\"{}\"", tag, old)
        } else {
            format!("{}: \"{}\" \u{2192} \"{}\"", tag, old, new)
        };
        let display = truncate_right(&change, max_width);
        lines.push(Line::from(Span::styled(
            display,
            Style::default().fg(Color::Magenta),
        )));
    }
    lines
}

// ============================================================================
// Mutation Introspection
// ============================================================================

/// Extract pending tag edits from a list of mutations.
pub fn pending_edits_from_mutations(
    mutations: &[mm_meta::mutations::Mutation],
) -> std::collections::HashMap<i64, Vec<(String, String, String)>> {
    use mm_meta::mutations::Mutation;
    let mut result: std::collections::HashMap<i64, Vec<(String, String, String)>> =
        std::collections::HashMap::new();

    for mutation in mutations {
        if let Mutation::ApplyTagOps(ref ops) = mutation {
            for op in &ops.ops {
                let old = op.old_value.clone().unwrap_or_default();
                let new = op.new_value.clone().unwrap_or_default();
                result
                    .entry(op.inode)
                    .or_default()
                    .push((op.tag_name.clone(), old, new));
            }
        }
    }

    result
}

