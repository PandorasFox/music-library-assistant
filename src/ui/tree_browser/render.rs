//! Rendering
//!
//! Unified rendering for the tree browser.
//! Dispatches to variant-specific layouts while sharing common tree rendering.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::widgets::{LateralView, UnifiedTitleBar};

use super::config::TreeBrowserConfig;
use super::entry::TreeEntry;
use super::navigator::TreeNavigator;
use super::variants::{BrowserVariant, CorpusBrowserVariant, DirectorySelectorVariant};

/// Render the tree browser.
pub fn render(
    f: &mut Frame,
    area: Rect,
    config: &TreeBrowserConfig,
    nav: &mut TreeNavigator,
    variant: &mut BrowserVariant,
) {
    match variant {
        BrowserVariant::CorpusBrowser(v) => render_corpus_browser(f, area, config, nav, v),
        BrowserVariant::DirectorySelector(v) => render_directory_selector(f, area, config, nav, v),
    }
}

/// Render corpus browser layout (titlebar + search bar + tree).
fn render_corpus_browser(
    f: &mut Frame,
    area: Rect,
    _config: &TreeBrowserConfig,
    nav: &mut TreeNavigator,
    variant: &mut CorpusBrowserVariant,
) {
    // Layout: Title bar | Search bar | Tree (full width)
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(UnifiedTitleBar::height()), // Title bar (3 lines)
            Constraint::Length(3),                          // Search bar (3 lines: border + content + border)
            Constraint::Min(5),                             // Tree browser
        ])
        .split(area);

    // Title bar (part of lateral ring)
    let titlebar = UnifiedTitleBar::new(LateralView::CorpusBrowser);
    titlebar.render(f, main_chunks[0]);

    // Search bar (persistent)
    variant.render_search_bar(f, main_chunks[1]);

    // Tree pane at full width
    render_tree_pane(f, main_chunks[2], nav, variant, true);

    // Overlays (match selection modal)
    variant.render_overlays(f, area);
}

/// Render directory selector layout (header + tree + footer).
fn render_directory_selector(
    f: &mut Frame,
    area: Rect,
    config: &TreeBrowserConfig,
    nav: &mut TreeNavigator,
    variant: &mut DirectorySelectorVariant,
) {
    // Layout: Header | Tree | Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Min(5),    // Tree
            Constraint::Length(3), // Footer
        ])
        .split(area);

    render_selector_header(f, chunks[0], config, variant);
    render_tree_pane(f, chunks[1], nav, variant, false);
    render_selector_footer(f, chunks[2]);
}

/// Render the tree pane (shared logic with variant-specific entry formatting).
fn render_tree_pane<V>(
    f: &mut Frame,
    area: Rect,
    nav: &mut TreeNavigator,
    variant: &V,
    is_corpus: bool,
) where
    V: TreeEntryRenderer,
{
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
        .map(|(idx, entry)| variant.render_entry_line(entry, idx == cursor_idx))
        .collect();

    let title = format!(
        "{} [{}/{}]",
        if is_corpus { "Files" } else { "Directories" },
        cursor_idx + 1,
        entries.len()
    );

    let block = Block::default().borders(Borders::ALL).title(title);
    let paragraph = Paragraph::new(lines).block(block);
    f.render_widget(paragraph, area);
}

/// Render selector header.
fn render_selector_header(
    f: &mut Frame,
    area: Rect,
    config: &TreeBrowserConfig,
    variant: &DirectorySelectorVariant,
) {
    let selection_count = variant.selection_count();

    // Add warning indicators for large selections
    let warning_indicators = if selection_count >= 5 {
        " ⚠".repeat(selection_count / 5)
    } else {
        String::new()
    };

    let title_line = Line::from(vec![
        Span::styled(
            &config.title,
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(&warning_indicators),
    ]);

    let info_line = if selection_count > 5 {
        Line::styled(
            format!("{} directories selected - large operation", selection_count),
            Style::default().fg(Color::Yellow),
        )
    } else if selection_count > 0 {
        Line::raw(format!("{} directories selected", selection_count))
    } else {
        Line::styled(
            "Space to select, Enter to proceed",
            Style::default().fg(Color::DarkGray),
        )
    };

    let block = Block::default().borders(Borders::ALL);
    let paragraph = Paragraph::new(vec![title_line, info_line]).block(block);
    f.render_widget(paragraph, area);
}

/// Render selector footer with keyboard hints.
fn render_selector_footer(f: &mut Frame, area: Rect) {
    let hints = Line::from(vec![
        Span::styled("↑↓", Style::default().fg(Color::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("←→", Style::default().fg(Color::Cyan)),
        Span::raw(" expand/collapse  "),
        Span::styled("Space", Style::default().fg(Color::Cyan)),
        Span::raw(" toggle  "),
        Span::styled("A", Style::default().fg(Color::Cyan)),
        Span::raw(" all  "),
        Span::styled("N", Style::default().fg(Color::Cyan)),
        Span::raw(" none  "),
        Span::styled("Enter", Style::default().fg(Color::Cyan)),
        Span::raw(" proceed"),
    ]);

    let block = Block::default().borders(Borders::ALL);
    let paragraph = Paragraph::new(hints).block(block);
    f.render_widget(paragraph, area);
}

// ============================================================================
// Entry Rendering Trait
// ============================================================================

/// Trait for rendering tree entries.
trait TreeEntryRenderer {
    fn render_entry_line(&self, entry: &TreeEntry, is_cursor: bool) -> Line<'static>;
}

impl TreeEntryRenderer for CorpusBrowserVariant {
    fn render_entry_line(&self, entry: &TreeEntry, is_cursor: bool) -> Line<'static> {
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
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
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
}

impl TreeEntryRenderer for DirectorySelectorVariant {
    fn render_entry_line(&self, entry: &TreeEntry, is_cursor: bool) -> Line<'static> {
        let indent = "  ".repeat(entry.depth);

        let selection_marker = self.selection_marker(entry);
        let is_ancestor = self.is_ancestor_of_selected(&entry.path);

        let expand_indicator = if entry.has_children {
            if entry.is_expanded {
                "▼ "
            } else {
                "▶ "
            }
        } else {
            "  "
        };

        let count_suffix = if self.config().show_item_counts && entry.item_count > 0 {
            format!("  ({} tracks)", entry.item_count)
        } else {
            String::new()
        };

        // Styling
        let marker_style = if self.is_selected(&entry.path) {
            Style::default().fg(Color::Green)
        } else if is_ancestor {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };

        let name_style = if is_cursor {
            Style::default()
                .bg(Color::DarkGray)
                .fg(Color::White)
                .add_modifier(Modifier::BOLD)
        } else if is_ancestor {
            Style::default().fg(Color::DarkGray)
        } else {
            Style::default()
        };

        let expand_style = Style::default().fg(Color::Yellow);
        let count_style = Style::default().fg(Color::DarkGray);

        Line::from(vec![
            Span::raw(indent),
            Span::styled(format!("{} ", selection_marker), marker_style),
            Span::styled(expand_indicator, expand_style),
            Span::styled(entry.name.clone(), name_style),
            Span::styled(count_suffix, count_style),
        ])
    }
}
