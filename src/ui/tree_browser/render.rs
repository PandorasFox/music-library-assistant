//! Rendering
//!
//! Unified rendering for the tree browser.
//! Dispatches to variant-specific layouts while sharing common tree rendering.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::widgets::{LateralView, UnifiedTitleBar, CURSOR_STYLE};

use super::entry::TreeEntry;
use super::navigator::TreeNavigator;
use super::variants::BrowserVariant;

/// Render the tree browser.
pub fn render(
    f: &mut Frame,
    area: Rect,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
) {
    // Currently only CorpusBrowser variant exists
    render_corpus_browser(f, area, nav, variant);
}

/// Render corpus browser layout (titlebar + filter bar + tree).
fn render_corpus_browser(
    f: &mut Frame,
    area: Rect,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
) {
    // Layout: Title bar | Filter bar | Tree (full width)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()), // Title bar (3 lines)
            Constraint::Length(3),                          // Filter bar (3 lines: border + content + border)
            Constraint::Min(5),                             // Tree browser
        ])
        .split(area);

    // Title bar (part of lateral ring)
    let titlebar = UnifiedTitleBar::new(LateralView::CorpusBrowser);
    titlebar.render(f, main_chunks[0]);

    // Filter bar - show filter status or hint
    render_filter_bar(f, main_chunks[1], nav);

    // Tree pane at full width
    render_tree_pane(f, main_chunks[2], nav);

    // Overlays (match selection modal)
    variant.render_overlays(f, area);
}

/// Render the filter status bar for corpus browser.
fn render_filter_bar(f: &mut Frame, area: Rect, nav: &TreeNavigator) {
    let (content, style) = if nav.has_path_filter() {
        // Active filter - show count and hint to clear
        let count = nav.filtered_file_count().unwrap_or(0);
        (
            format!("Filtered: {} files  (Ctrl+F to change, Esc to clear)", count),
            Style::default().fg(Color::Green),
        )
    } else {
        // No filter - show hint
        (
            "Press Ctrl+F to filter files".to_string(),
            Style::default().fg(Color::DarkGray),
        )
    };

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray))
        .title("Filter");

    let paragraph = Paragraph::new(Span::styled(content, style)).block(block);
    f.render_widget(paragraph, area);
}

/// Render the tree pane.
fn render_tree_pane(f: &mut Frame, area: Rect, nav: &mut TreeNavigator) {
    let inner_height = area.height.saturating_sub(2) as usize;
    nav.set_visible_height(inner_height);

    let entries = nav.entries();
    let cursor_idx = nav.cursor_idx();
    let scroll = nav.scroll_offset();

    let lines: Vec<Line> = entries
        .iter()
        .enumerate()
        .skip(scroll)
        .take(inner_height)
        .map(|(idx, entry)| render_entry_line(entry, idx == cursor_idx))
        .collect();

    let title = format!("Files [{}/{}]", cursor_idx + 1, entries.len());

    let block = Block::default().borders(Borders::ALL).title(title);
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Render a single tree entry line.
fn render_entry_line(entry: &TreeEntry, is_cursor: bool) -> Line<'static> {
    let indent = "  ".repeat(entry.depth);

    let expand_indicator = if entry.is_directory {
        if entry.has_children {
            if entry.is_expanded {
                "▼ "
            } else {
                "▶ "
            }
        } else {
            "  "
        }
    } else {
        "  "
    };

    let icon = if entry.is_directory { "" } else { "♪ " };

    let count_suffix = if entry.is_directory && entry.item_count > 0 {
        format!("  ({} tracks)", entry.item_count)
    } else {
        String::new()
    };

    let base_style = if is_cursor {
        CURSOR_STYLE
    } else if entry.is_directory {
        Style::default().fg(Color::Blue)
    } else {
        Style::default().fg(Color::White)
    };

    let expand_style = Style::default().fg(Color::Yellow);
    let count_style = Style::default().fg(Color::DarkGray);

    Line::from(vec![
        Span::raw(indent),
        Span::styled(expand_indicator, expand_style),
        Span::styled(format!("{}{}", icon, entry.name), base_style),
        Span::styled(count_suffix, count_style),
    ])
}
