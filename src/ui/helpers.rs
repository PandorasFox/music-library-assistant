//! Shared UI rendering utilities
//!
//! Common helpers used across multiple UI modules to avoid code duplication.

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
