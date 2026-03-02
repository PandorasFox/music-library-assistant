//! Tabbed Transaction Review — persistent lateral tab wrapping the shared
//! transaction review core.
//!
//! Thin wrapper: adds Tab/BackTab cycling, delegates everything else to
//! `TransactionReviewState`.

use crate::ui::input::InputAction;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::ui::transaction_review::{
    self, DecisionSummary, TransactionReviewAction, TransactionReviewState,
};

// ============================================================================
// State
// ============================================================================

/// State for the tabbed transaction review lateral view.
pub struct TabbedTransactionReviewState {
    pub review: TransactionReviewState,
}

impl TabbedTransactionReviewState {
    pub fn new() -> Self {
        Self {
            review: TransactionReviewState::new_tabbed(),
        }
    }

    pub fn handle_input(&mut self, action: &InputAction) -> TabbedTransactionReviewAction {
        // Intercept Tab/BackTab for lateral cycling
        match action {
            InputAction::CycleNext => return TabbedTransactionReviewAction::CycleNext,
            InputAction::CyclePrev => return TabbedTransactionReviewAction::CyclePrev,
            _ => {}
        }

        // Delegate everything else to the core
        match self.review.handle_input(action) {
            TransactionReviewAction::Cancel => TabbedTransactionReviewAction::RequestQuit,
            other => TabbedTransactionReviewAction::Review(other),
        }
    }
}

// ============================================================================
// Actions
// ============================================================================

/// Actions produced by the tabbed transaction review.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabbedTransactionReviewAction {
    CycleNext,
    CyclePrev,
    RequestQuit,
    /// Core action delegated from TransactionReviewState.
    Review(TransactionReviewAction),
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the tabbed transaction review (full-area, no modal frame).
pub fn render(
    f: &mut Frame,
    area: Rect,
    state: &TabbedTransactionReviewState,
    decisions: &[DecisionSummary],
) {
    if decisions.is_empty() {
        let empty = Paragraph::new("No decisions staged")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(empty, area);
    } else {
        transaction_review::render_content(f, area, &state.review, decisions);
    }

    transaction_review::render_removal_popup(f, area, &state.review, decisions);
}
