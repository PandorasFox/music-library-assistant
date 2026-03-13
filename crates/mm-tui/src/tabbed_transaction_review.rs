//! Tabbed Transaction Review — persistent lateral tab wrapping the shared
//! transaction review core.
//!
//! Thin wrapper: adds Tab/BackTab cycling, delegates everything else to
//! `TransactionReviewState`.

use crate::input::InputAction;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::transaction_review::{self, TransactionReviewAction, TransactionReviewState};

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

    /// Handle input. CycleNext/CyclePrev are intercepted centrally by `App::handle_input`.
    /// Cancel maps to `None` (centralized cancel-as-quit fires).
    pub fn handle_input(&mut self, action: &InputAction) -> Option<TabbedTransactionReviewAction> {
        match self.review.handle_input(action) {
            TransactionReviewAction::Cancel => None, // Centralized cancel-as-quit handles this
            TransactionReviewAction::None => None,
            other => Some(TabbedTransactionReviewAction::Review(other)),
        }
    }
}

// ============================================================================
// Actions
// ============================================================================

/// Domain actions produced by the tabbed transaction review.
///
/// Protocol actions (CycleNext, CyclePrev, Cancel-as-quit) are handled centrally.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TabbedTransactionReviewAction {
    /// Core action delegated from TransactionReviewState.
    Review(TransactionReviewAction),
}

// ============================================================================
// Rendering
// ============================================================================

/// Render the tabbed transaction review (full-area, no modal frame).
pub fn render(f: &mut Frame, area: Rect, state: &mut TabbedTransactionReviewState) {
    if state.review.decisions.is_empty() {
        let empty = Paragraph::new("No decisions staged")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center)
            .block(Block::default().borders(Borders::NONE));
        f.render_widget(empty, area);
    } else {
        transaction_review::render_content(f, area, &mut state.review);
    }

    transaction_review::render_removal_popup(f, area, &state.review);
}
