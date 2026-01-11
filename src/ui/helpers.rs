//! Shared UI rendering utilities
//!
//! Common helpers used across multiple UI modules to avoid code duplication.
//!
//! NOTE: Some helpers are not yet used but available for future features.

#![allow(dead_code)]

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState},
    Frame,
};

/// Default highlight style for menu selections
pub const HIGHLIGHT_STYLE: Style = Style::new().bg(Color::DarkGray);

/// Default highlight symbol for menu selections
pub const HIGHLIGHT_SYMBOL: &str = ">> ";

/// Create a centered rectangle within a parent area.
///
/// Used for modal dialogs and popups.
pub fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}

/// Render a standard selectable list with highlighting.
///
/// This eliminates the repeated pattern of creating List widgets throughout the codebase.
pub fn render_selectable_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: &[&str],
    state: &mut ListState,
) {
    let list_items: Vec<ListItem> = items
        .iter()
        .map(|item| ListItem::new(Line::from(*item)))
        .collect();

    let list = List::new(list_items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(HIGHLIGHT_STYLE.add_modifier(Modifier::BOLD))
        .highlight_symbol(HIGHLIGHT_SYMBOL);

    f.render_stateful_widget(list, area, state);
}

/// Render a static (non-selectable) list for display purposes.
pub fn render_static_list(f: &mut Frame, area: Rect, title: &str, items: &[String]) {
    let list_items: Vec<ListItem> = items
        .iter()
        .map(|item| ListItem::new(Line::from(item.as_str())))
        .collect();

    let list = List::new(list_items)
        .block(Block::default().borders(Borders::ALL).title(title));

    f.render_widget(list, area);
}

/// Render a selectable list with styled items.
///
/// Each item can have its own style (e.g., for color-coding different types).
pub fn render_styled_list(
    f: &mut Frame,
    area: Rect,
    title: &str,
    items: Vec<(String, Style)>,
    state: &mut ListState,
) {
    let list_items: Vec<ListItem> = items
        .into_iter()
        .map(|(text, style)| ListItem::new(Line::from(text)).style(style))
        .collect();

    let list = List::new(list_items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(HIGHLIGHT_STYLE.add_modifier(Modifier::BOLD))
        .highlight_symbol(HIGHLIGHT_SYMBOL);

    f.render_stateful_widget(list, area, state);
}

/// Clear background for modal overlay.
pub fn clear_background(f: &mut Frame, area: Rect) {
    let clear_block = Block::default().style(Style::default().bg(Color::Reset));
    f.render_widget(clear_block, area);
}

/// Render a modal frame and return the inner area for content.
///
/// Creates a bordered box with a title, suitable for modal dialogs.
pub fn render_modal_frame(f: &mut Frame, area: Rect, title: &str, border_color: Color) -> Rect {
    let modal_block = Block::default()
        .borders(Borders::ALL)
        .title(title)
        .border_style(Style::default().fg(border_color));

    let inner = modal_block.inner(area);
    f.render_widget(modal_block, area);
    inner
}

/// Simple three-pane horizontal layout.
///
/// Returns (left, middle, right) areas.
pub fn three_pane_layout(area: Rect, left_width: u16, right_width: u16) -> (Rect, Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage(left_width),
            Constraint::Percentage(100 - left_width - right_width),
            Constraint::Percentage(right_width),
        ])
        .split(area);

    (chunks[0], chunks[1], chunks[2])
}

/// Standard vertical layout with header, content, and footer.
///
/// Returns (header, content, footer) areas.
pub fn standard_layout(area: Rect) -> (Rect, Rect, Rect) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(0),    // Content
            Constraint::Length(3), // Footer/Status
        ])
        .split(area);

    (chunks[0], chunks[1], chunks[2])
}

// ============================================================================
// Formatting Utilities
// ============================================================================

/// Truncate a file path for display, UTF-8 safe.
///
/// If the path is longer than `max_len`, it will be truncated from the left
/// with "..." prefix.
pub fn truncate_path_display(path: &str, max_len: usize) -> String {
    if path.chars().count() <= max_len {
        return path.to_string();
    }

    // Take the last max_len-3 characters
    let chars: Vec<char> = path.chars().collect();
    let start = chars.len().saturating_sub(max_len - 3);
    let truncated: String = chars[start..].iter().collect();
    format!("...{}", truncated)
}

/// Format bytes using binary SI units (KiB, MiB, GiB, TiB).
pub fn format_bytes_binary(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    const MIB: f64 = KIB * 1024.0;
    const GIB: f64 = MIB * 1024.0;
    const TIB: f64 = GIB * 1024.0;

    let bytes_f = bytes as f64;
    if bytes_f >= TIB {
        format!("{:.2} TiB", bytes_f / TIB)
    } else if bytes_f >= GIB {
        format!("{:.2} GiB", bytes_f / GIB)
    } else if bytes_f >= MIB {
        format!("{:.1} MiB", bytes_f / MIB)
    } else if bytes_f >= KIB {
        format!("{:.0} KiB", bytes_f / KIB)
    } else {
        format!("{} B", bytes)
    }
}

/// Format ETA as mm:ss or h:mm:ss.
pub fn format_eta(seconds: u64) -> String {
    if seconds >= 3600 {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        let secs = seconds % 60;
        format!("{}:{:02}:{:02}", hours, mins, secs)
    } else {
        let mins = seconds / 60;
        let secs = seconds % 60;
        format!("{}:{:02}", mins, secs)
    }
}

/// Calculate rolling average throughput in MiB/s from recent samples.
///
/// Uses samples from the last `window_secs` seconds.
pub fn calculate_rolling_throughput(
    samples: &std::collections::VecDeque<(std::time::Instant, u64)>,
    window_secs: u64,
) -> Option<f64> {
    if samples.len() < 2 {
        return None;
    }

    let now = std::time::Instant::now();
    let window_start = now - std::time::Duration::from_secs(window_secs);

    // Find samples within the window
    let samples_in_window: Vec<_> = samples
        .iter()
        .filter(|(t, _)| *t >= window_start)
        .collect();

    if samples_in_window.len() < 2 {
        return None;
    }

    let first = samples_in_window.first()?;
    let last = samples_in_window.last()?;

    let time_diff = last.0.duration_since(first.0).as_secs_f64();
    if time_diff < 0.1 {
        return None;
    }

    let bytes_diff = last.1.saturating_sub(first.1);
    let mib_per_sec = (bytes_diff as f64 / (1024.0 * 1024.0)) / time_diff;

    Some(mib_per_sec)
}
