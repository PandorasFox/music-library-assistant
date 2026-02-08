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
//! ┌─────────────────────────────────────┐┌──────────────┐
//! │ Corpus Browser | Insights | Deploy  ││  mla beta 1  │
//! └─────────────────────────────────────┘└──────────────┘
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
///
/// Note: Deploy was removed from the lateral ring - it's now accessed via
/// the Insights view by pressing Enter on deploy-related insights.
/// FormatStandardization was also removed - handled by ShitFormat signal flow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LateralView {
    TagSearch,
    CorpusBrowser,
    Insights,
}

impl LateralView {
    /// Display label for this view
    pub fn label(&self) -> &'static str {
        match self {
            LateralView::TagSearch => "Tag Search",
            LateralView::CorpusBrowser => "Corpus Browser",
            LateralView::Insights => "Insights & Operations",
        }
    }

    /// Get the next view in the ring (Tab)
    pub fn next(&self) -> Self {
        match self {
            LateralView::TagSearch => LateralView::CorpusBrowser,
            LateralView::CorpusBrowser => LateralView::Insights,
            LateralView::Insights => LateralView::TagSearch,
        }
    }

    /// Get the previous view in the ring (Shift-Tab)
    pub fn prev(&self) -> Self {
        match self {
            LateralView::TagSearch => LateralView::Insights,
            LateralView::CorpusBrowser => LateralView::TagSearch,
            LateralView::Insights => LateralView::CorpusBrowser,
        }
    }

    /// All views in order
    pub fn all() -> &'static [LateralView] {
        &[LateralView::TagSearch, LateralView::CorpusBrowser, LateralView::Insights]
    }
}

/// Unified title bar combining tab switcher and minimal title.
///
/// Two bordered panes side by side:
/// - Left: Tab switcher showing all views with current highlighted
/// - Right: App title "mla beta 1" (16 chars wide)
pub struct UnifiedTitleBar {
    current_view: LateralView,
}

impl UnifiedTitleBar {
    /// Create a new unified title bar
    pub fn new(current_view: LateralView) -> Self {
        Self { current_view }
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
        let paragraph = Paragraph::new("mla beta 1")
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
        // Test forward cycling: TagSearch → CorpusBrowser → Insights → TagSearch
        let view = LateralView::TagSearch;
        assert_eq!(view.next(), LateralView::CorpusBrowser);
        assert_eq!(view.next().next(), LateralView::Insights);
        assert_eq!(view.next().next().next(), LateralView::TagSearch);

        // Test backward cycling from CorpusBrowser
        let view = LateralView::CorpusBrowser;
        assert_eq!(view.prev(), LateralView::TagSearch);
        assert_eq!(view.prev().prev(), LateralView::Insights);
    }

    #[test]
    fn test_lateral_view_labels() {
        assert_eq!(LateralView::TagSearch.label(), "Tag Search");
        assert_eq!(LateralView::CorpusBrowser.label(), "Corpus Browser");
        assert_eq!(LateralView::Insights.label(), "Insights & Operations");
    }

    #[test]
    fn test_unified_titlebar_height() {
        assert_eq!(UnifiedTitleBar::height(), 3);
    }
}
