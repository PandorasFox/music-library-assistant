//! Tabbed Signal List Widget
//!
//! A horizontal tab bar with a scrollable list below for deploy signals.
//! Used in the deploy modal to show different signal categories.

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};
use crate::ui::helpers::render_pane;

/// The active tab in the deploy signal view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeployTab {
    #[default]
    Healthy,
    New,
    Conflicts,
    Leftover,
    Stale,
}

impl DeployTab {
    /// Display label for this tab.
    pub fn label(&self) -> &'static str {
        match self {
            DeployTab::Healthy => "Healthy",
            DeployTab::New => "New",
            DeployTab::Conflicts => "Conflicts",
            DeployTab::Leftover => "Leftover",
            DeployTab::Stale => "Stale",
        }
    }

    /// Get the next tab (Right arrow).
    pub fn next(&self) -> Self {
        match self {
            DeployTab::Healthy => DeployTab::New,
            DeployTab::New => DeployTab::Conflicts,
            DeployTab::Conflicts => DeployTab::Leftover,
            DeployTab::Leftover => DeployTab::Stale,
            DeployTab::Stale => DeployTab::Healthy,
        }
    }

    /// Get the previous tab (Left arrow).
    pub fn prev(&self) -> Self {
        match self {
            DeployTab::Healthy => DeployTab::Stale,
            DeployTab::New => DeployTab::Healthy,
            DeployTab::Conflicts => DeployTab::New,
            DeployTab::Leftover => DeployTab::Conflicts,
            DeployTab::Stale => DeployTab::Leftover,
        }
    }

    /// All tabs in order.
    pub fn all() -> &'static [DeployTab] {
        &[
            DeployTab::Healthy,
            DeployTab::New,
            DeployTab::Conflicts,
            DeployTab::Leftover,
            DeployTab::Stale,
        ]
    }

    /// Get the index of this tab (for scroll position array).
    pub fn index(&self) -> usize {
        match self {
            DeployTab::Healthy => 0,
            DeployTab::New => 1,
            DeployTab::Conflicts => 2,
            DeployTab::Leftover => 3,
            DeployTab::Stale => 4,
        }
    }
}

/// A tabbed list widget showing deploy signals.
///
/// # Layout
/// ```text
/// ┌──────────────────────────────────────┐
/// │ Healthy | New | Conflicts | ...      │
/// ├──────────────────────────────────────┤
/// │ /path/to/file1.flac                  │
/// │ /path/to/file2.flac                  │
/// │ ...                                  │
/// └──────────────────────────────────────┘
/// ```
pub struct TabbedSignalList<'a> {
    /// Currently active tab.
    active_tab: DeployTab,
    /// Items to display in the list (paths).
    items: Vec<&'a str>,
    /// Current scroll offset.
    scroll: usize,
    /// Counts per tab for display in tab labels.
    tab_counts: [usize; 5],
}

impl<'a> TabbedSignalList<'a> {
    pub fn new(active_tab: DeployTab) -> Self {
        Self {
            active_tab,
            items: Vec::new(),
            scroll: 0,
            tab_counts: [0; 5],
        }
    }

    /// Set the items to display (paths sorted alphabetically).
    pub fn items(mut self, items: Vec<&'a str>) -> Self {
        self.items = items;
        self
    }

    /// Set the scroll offset.
    pub fn scroll(mut self, offset: usize) -> Self {
        self.scroll = offset;
        self
    }

    /// Set counts for all tabs (for display in tab labels).
    pub fn tab_counts(mut self, counts: [usize; 5]) -> Self {
        self.tab_counts = counts;
        self
    }

    /// Render the widget.
    pub fn render(self, f: &mut Frame, area: Rect) {
        // Split: tab bar (3 lines) + list (remaining)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Tab bar with borders
                Constraint::Min(1),    // List
            ])
            .split(area);

        // Render tab bar
        self.render_tabs(f, chunks[0]);

        // Render list
        self.render_list(f, chunks[1]);
    }

    fn render_tabs(&self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();

        for (i, tab) in DeployTab::all().iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            }

            let count = self.tab_counts[tab.index()];
            let label = format!("{} ({})", tab.label(), count);

            let style = if *tab == self.active_tab {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if count > 0 {
                // Highlight tabs with items
                Style::default().fg(Color::White)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            spans.push(Span::styled(label, style));
        }

        let paragraph = Paragraph::new(Line::from(spans)).block(
            Block::default()
                .borders(Borders::ALL)
                .title("Signal Categories"),
        );

        f.render_widget(paragraph, area);
    }

    fn render_list(&self, f: &mut Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!("{} Files", self.active_tab.label()));

        let inner = render_pane(f, area, block);
        let visible_height = inner.height as usize;

        // Calculate scroll bounds
        let max_scroll = self.items.len().saturating_sub(visible_height);
        let clamped_scroll = self.scroll.min(max_scroll);

        // Create list items with selection highlighting
        let items: Vec<ListItem> = self
            .items
            .iter()
            .enumerate()
            .skip(clamped_scroll)
            .take(visible_height)
            .map(|(idx, path)| {
                let is_selected = idx == self.scroll;
                let style = if is_selected {
                    CURSOR_STYLE
                } else {
                    LIST_ITEM_STYLE
                };
                ListItem::new(Line::from(Span::styled(*path, style)))
            })
            .collect();

        let list = List::new(items);

        f.render_widget(list, inner);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deploy_tab_cycling() {
        let tab = DeployTab::Healthy;
        assert_eq!(tab.next(), DeployTab::New);
        assert_eq!(tab.next().next(), DeployTab::Conflicts);
        assert_eq!(tab.prev(), DeployTab::Stale);
    }

    #[test]
    fn test_deploy_tab_index() {
        assert_eq!(DeployTab::Healthy.index(), 0);
        assert_eq!(DeployTab::New.index(), 1);
        assert_eq!(DeployTab::Conflicts.index(), 2);
        assert_eq!(DeployTab::Leftover.index(), 3);
        assert_eq!(DeployTab::Stale.index(), 4);
    }
}
