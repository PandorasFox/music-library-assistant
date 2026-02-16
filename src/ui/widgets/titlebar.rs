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
//! │ Tag Search | Corpus Browser | ... | Deploy   ││  mm beta 6  │
//! └──────────────────────────────────────────────┘└──────────────┘
//! ```

use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

/// Width of the title pane (right side)
const TITLE_PANE_WIDTH: u16 = 16;

/// Views available in the lateral view ring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateralView {
    Config,
    TagSearch,
    CorpusBrowser,
    Insights,
    Inbox,
    Deploy,
}

impl LateralView {
    /// Display label for this view
    pub fn label(&self) -> &'static str {
        match self {
            LateralView::Config => "Config",
            LateralView::TagSearch => "Tag Search",
            LateralView::CorpusBrowser => "Corpus Browser",
            LateralView::Insights => "Insights & Operations",
            LateralView::Inbox => "Inbox",
            LateralView::Deploy => "Deploy",
        }
    }

    /// Get the next view in the ring (Tab)
    pub fn next(&self) -> Self {
        match self {
            LateralView::Config => LateralView::TagSearch,
            LateralView::TagSearch => LateralView::CorpusBrowser,
            LateralView::CorpusBrowser => LateralView::Insights,
            LateralView::Insights => LateralView::Inbox,
            LateralView::Inbox => LateralView::Deploy,
            LateralView::Deploy => LateralView::Config,
        }
    }

    /// Get the previous view in the ring (Shift-Tab)
    pub fn prev(&self) -> Self {
        match self {
            LateralView::Config => LateralView::Deploy,
            LateralView::TagSearch => LateralView::Config,
            LateralView::CorpusBrowser => LateralView::TagSearch,
            LateralView::Insights => LateralView::CorpusBrowser,
            LateralView::Inbox => LateralView::Insights,
            LateralView::Deploy => LateralView::Inbox,
        }
    }

    /// All views in order
    pub fn all() -> &'static [LateralView] {
        &[LateralView::Config, LateralView::TagSearch, LateralView::CorpusBrowser, LateralView::Insights, LateralView::Inbox, LateralView::Deploy]
    }
}

/// Unified title bar combining tab switcher and minimal title.
///
/// Two bordered panes side by side:
/// - Left: Tab switcher showing all views with current highlighted
/// - Right: App title "mm beta 6" (16 chars wide)
pub struct UnifiedTitleBar {
    current_view: LateralView,
    /// When true and Deploy tab is not active, render Deploy label in Magenta.
    deploy_needs_action: bool,
}

impl UnifiedTitleBar {
    /// Create a new unified title bar
    pub fn new(current_view: LateralView) -> Self {
        Self { current_view, deploy_needs_action: false }
    }

    /// Set whether the Deploy tab should be highlighted (purple) when not active.
    pub fn with_deploy_needs_action(mut self, needs_action: bool) -> Self {
        self.deploy_needs_action = needs_action;
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
                Constraint::Min(20),                // Tab switcher (fills remaining)
                Constraint::Length(TITLE_PANE_WIDTH), // Title (fixed)
            ])
            .split(area);

        // Render tab switcher pane (left)
        self.render_tab_switcher(f, chunks[0]);

        // Render title pane (right)
        self.render_title(f, chunks[1]);
    }

    fn render_tab_switcher(&self, f: &mut Frame, area: Rect) {
        let mut spans = Vec::new();

        for (i, view) in LateralView::all().iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" | ", Style::default().fg(Color::DarkGray)));
            }

            let label = view.label();
            let style = if *view == self.current_view {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if *view == LateralView::Deploy && self.deploy_needs_action {
                // Purple highlight for Deploy when there's work to do
                Style::default().fg(Color::Magenta)
            } else {
                Style::default().fg(Color::DarkGray)
            };

            spans.push(Span::styled(label, style));
        }

        let paragraph = Paragraph::new(Line::from(spans))
            .block(Block::default().borders(Borders::ALL));

        f.render_widget(paragraph, area);
    }

    fn render_title(&self, f: &mut Frame, area: Rect) {
        let paragraph = Paragraph::new("mm beta 6")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)));

        f.render_widget(paragraph, area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lateral_view_cycling() {
        // Test forward cycling: Config → TagSearch → CorpusBrowser → Insights → Inbox → Deploy → Config
        let view = LateralView::Config;
        assert_eq!(view.next(), LateralView::TagSearch);
        assert_eq!(view.next().next(), LateralView::CorpusBrowser);
        assert_eq!(view.next().next().next(), LateralView::Insights);
        assert_eq!(view.next().next().next().next(), LateralView::Inbox);
        assert_eq!(view.next().next().next().next().next(), LateralView::Deploy);
        assert_eq!(view.next().next().next().next().next().next(), LateralView::Config);

        // Test backward cycling from TagSearch
        let view = LateralView::TagSearch;
        assert_eq!(view.prev(), LateralView::Config);
        assert_eq!(view.prev().prev(), LateralView::Deploy);
        assert_eq!(view.prev().prev().prev(), LateralView::Inbox);
    }

    #[test]
    fn test_lateral_view_labels() {
        assert_eq!(LateralView::Config.label(), "Config");
        assert_eq!(LateralView::TagSearch.label(), "Tag Search");
        assert_eq!(LateralView::CorpusBrowser.label(), "Corpus Browser");
        assert_eq!(LateralView::Insights.label(), "Insights & Operations");
        assert_eq!(LateralView::Deploy.label(), "Deploy");
    }

    #[test]
    fn test_unified_titlebar_height() {
        assert_eq!(UnifiedTitleBar::height(), 3);
    }
}
