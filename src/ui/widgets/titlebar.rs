//! Title Bar and Tab Switcher Widgets
//!
//! Unified title bar for lateral view ring navigation.
//!
//! ## Components
//!
//! - `UnifiedTitleBar`: Two-pane header with tab switcher and app title
//!
//! ## Layout
//!
//! ```text
//! ┌──────────────────────────────────────────────┐┌──────────────┐
//! │ Search | Files | Health | ... | History | ...  ││  mm beta 9  │
//! └──────────────────────────────────────────────┘└──────────────┘
//! ```

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// App title text displayed in the right pane
const TITLE_TEXT: &str = "mm beta 9";

/// Width of the title pane: text + 1 padding each side + 2 border
const TITLE_PANE_WIDTH: u16 = TITLE_TEXT.len() as u16 + 4;

/// Views available in the lateral view ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateralView {
    Config,
    Search,
    Files,
    Health,
    History,
    Transaction,
    Inbox,
    Deploy,
    ExternalMatches,
}

impl LateralView {
    /// Display label for this view
    pub fn label(&self) -> &'static str {
        match self {
            LateralView::Config => "Config",
            LateralView::Search => "Search",
            LateralView::Files => "Files",
            LateralView::Health => "Health",
            LateralView::History => "History",
            LateralView::Transaction => "Transaction",
            LateralView::Inbox => "Inbox",
            LateralView::Deploy => "Deploy",
            LateralView::ExternalMatches => "Ext. Matches",
        }
    }

    /// Get the next view in the ring (Tab)
    pub fn next(&self, transactions_open: bool) -> Self {
        match self {
            LateralView::Config => LateralView::Search,
            LateralView::Search => LateralView::Files,
            LateralView::Files => LateralView::Health,
            LateralView::Health => {
                if transactions_open {
                    LateralView::Transaction
                } else {
                    LateralView::Inbox
                }
            }
            LateralView::Transaction => LateralView::Inbox,
            LateralView::Inbox => LateralView::Deploy,
            LateralView::Deploy => LateralView::History,
            LateralView::History => LateralView::ExternalMatches,
            LateralView::ExternalMatches => LateralView::Config,
        }
    }

    /// Get the previous view in the ring (Shift-Tab)
    pub fn prev(&self, transactions_open: bool) -> Self {
        match self {
            LateralView::Config => LateralView::ExternalMatches,
            LateralView::Search => LateralView::Config,
            LateralView::Files => LateralView::Search,
            LateralView::Health => LateralView::Files,
            LateralView::Transaction => LateralView::Health,
            LateralView::Inbox => {
                if transactions_open {
                    LateralView::Transaction
                } else {
                    LateralView::Health
                }
            }
            LateralView::Deploy => LateralView::Inbox,
            LateralView::History => LateralView::Deploy,
            LateralView::ExternalMatches => LateralView::History,
        }
    }

    /// All views in order
    pub fn all(transactions_open: bool) -> Vec<LateralView> {
        let mut views = vec![
            LateralView::Config,
            LateralView::Search,
            LateralView::Files,
            LateralView::Health,
        ];
        if transactions_open {
            views.push(LateralView::Transaction);
        }
        views.push(LateralView::Inbox);
        views.push(LateralView::Deploy);
        views.push(LateralView::History);
        views.push(LateralView::ExternalMatches);
        views
    }
}

/// Unified title bar combining tab switcher and minimal title.
///
/// Two bordered panes side by side:
/// - Left: Tab switcher showing all views with current highlighted
/// - Right: App title (auto-sized to fit text)
pub struct UnifiedTitleBar {
    current_view: LateralView,
    /// When true and Deploy tab is not active, render Deploy label in Magenta.
    deploy_needs_action: bool,
    /// When true and Transaction tab is not active, render Transaction label in Magenta.
    transaction_has_decisions: bool,
    /// Whether the Transaction tab is visible in the ring.
    transactions_open: bool,
}

impl UnifiedTitleBar {
    /// Create a new unified title bar
    pub fn new(current_view: LateralView) -> Self {
        Self {
            current_view,
            deploy_needs_action: false,
            transaction_has_decisions: false,
            transactions_open: false,
        }
    }

    /// Set whether the Deploy tab should be highlighted (purple) when not active.
    pub fn with_deploy_needs_action(mut self, needs_action: bool) -> Self {
        self.deploy_needs_action = needs_action;
        self
    }

    /// Set whether the Transaction tab should be highlighted (magenta) when not active.
    pub fn with_transaction_has_decisions(mut self, has: bool) -> Self {
        self.transaction_has_decisions = has;
        self
    }

    /// Set whether the Transaction tab is visible in the ring.
    pub fn with_transactions_open(mut self, open: bool) -> Self {
        self.transactions_open = open;
        self
    }

    /// Get the height needed for this widget (3 for borders)
    pub fn height() -> u16 {
        3
    }

    /// Render the unified title bar.
    pub fn render(&self, f: &mut Frame, area: Rect) {
        if area.height < 3 || area.width < TITLE_PANE_WIDTH + 10 {
            return;
        }

        // Split: tabs on left (flexible), title on right (fixed 16 chars)
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([
                Constraint::Min(20),                  // Tab switcher (fills remaining)
                Constraint::Length(TITLE_PANE_WIDTH), // Title (fixed)
            ])
            .split(area);

        // Render tab switcher pane (left)
        self.render_tab_switcher(f, chunks[0]);

        // Render title pane (right)
        self.render_title(f, chunks[1]);
    }

    /// Compute click target rects for each tab label.
    ///
    /// Returns (LateralView, Rect) pairs for hit-testing mouse clicks.
    /// Must be called with the same `area` passed to `render()`.
    pub fn tab_click_rects(&self, area: Rect) -> Vec<(LateralView, Rect)> {
        if area.height < 3 || area.width < TITLE_PANE_WIDTH + 10 {
            return Vec::new();
        }

        // Same horizontal split as render()
        let chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(TITLE_PANE_WIDTH)])
            .split(area);

        let tab_area = chunks[0];
        // Inner area (inside Borders::ALL)
        let inner_x = tab_area.x + 1;
        let inner_y = tab_area.y + 1;

        let mut rects = Vec::new();
        let mut x = inner_x;

        let all_views = LateralView::all(self.transactions_open);
        for (i, view) in all_views.iter().enumerate() {
            if i > 0 {
                x += 3; // " | " separator
            }
            let label_len = view.label().len() as u16;
            rects.push((*view, Rect::new(x, inner_y, label_len, 1)));
            x += label_len;
        }

        rects
    }

    fn render_tab_switcher(&self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();
        let all_views = LateralView::all(self.transactions_open);

        for (i, view) in all_views.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            }

            let label = view.label();
            let style = if *view == self.current_view {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if *view == LateralView::Transaction && self.transaction_has_decisions {
                Style::default().fg(Color::Magenta)
            } else if *view == LateralView::Deploy && self.deploy_needs_action {
                // Purple highlight for Deploy when there's work to do
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            spans.push(Span::styled(label, style));
        }

        let paragraph =
            Paragraph::new(Line::from(spans)).block(Block::default().borders(Borders::ALL));

        f.render_widget(paragraph, area);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let paragraph = Paragraph::new(TITLE_TEXT)
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(Color::Cyan)),
            );

        f.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lateral_view_cycling() {
        // Test forward cycling without transactions: Config → Search → Files → Health → Inbox → Deploy → History → ExternalMatches → Config
        let view = LateralView::Config;
        assert_eq!(view.next(false), LateralView::Search);
        assert_eq!(view.next(false).next(false), LateralView::Files);
        assert_eq!(
            view.next(false).next(false).next(false),
            LateralView::Health
        );
        assert_eq!(
            view.next(false).next(false).next(false).next(false),
            LateralView::Inbox
        );
        assert_eq!(
            view.next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false),
            LateralView::Deploy
        );
        assert_eq!(
            view.next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false),
            LateralView::History
        );
        assert_eq!(
            view.next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false),
            LateralView::ExternalMatches
        );
        assert_eq!(
            view.next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false)
                .next(false),
            LateralView::Config
        );

        // Test backward cycling from Search
        let view = LateralView::Search;
        assert_eq!(view.prev(false), LateralView::Config);
        assert_eq!(view.prev(false).prev(false), LateralView::ExternalMatches);
        assert_eq!(
            view.prev(false).prev(false).prev(false),
            LateralView::History
        );

        // Test forward cycling with transactions: Health → Transaction → Inbox
        assert_eq!(LateralView::Health.next(true), LateralView::Transaction);
        assert_eq!(LateralView::Transaction.next(true), LateralView::Inbox);

        // Test backward cycling with transactions: Inbox → Transaction → Health
        assert_eq!(LateralView::Inbox.prev(true), LateralView::Transaction);
        assert_eq!(LateralView::Transaction.prev(true), LateralView::Health);
    }

    #[test]
    fn test_lateral_view_labels() {
        assert_eq!(LateralView::Config.label(), "Config");
        assert_eq!(LateralView::Search.label(), "Search");
        assert_eq!(LateralView::Files.label(), "Files");
        assert_eq!(LateralView::Health.label(), "Health");
        assert_eq!(LateralView::Deploy.label(), "Deploy");
    }

    #[test]
    fn test_unified_titlebar_height() {
        assert_eq!(UnifiedTitleBar::height(), 3);
    }
}
