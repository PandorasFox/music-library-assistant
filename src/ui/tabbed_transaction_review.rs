//! Tabbed Transaction Review — persistent lateral tab wrapping the shared
//! transaction review core.
//!
//! Thin wrapper: adds Tab/BackTab cycling, delegates everything else to
//! `TransactionReviewState`.

use crossterm::event::{KeyCode, KeyEvent};
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

    pub fn handle_key(&mut self, key: KeyEvent) -> TabbedTransactionReviewAction {
        // Intercept Tab/BackTab for lateral cycling
        match key.code {
            KeyCode::Tab => return TabbedTransactionReviewAction::CycleNext,
            KeyCode::BackTab => return TabbedTransactionReviewAction::CyclePrev,
            _ => {}
        }

        // Delegate everything else to the core
        match self.review.handle_key(key) {
            TransactionReviewAction::Cancel => {
                // No cancel in tabbed mode — Esc produces Cancel from core,
                // but here it's a no-op (tabbed view has no parent to pop to)
                TabbedTransactionReviewAction::None
            }
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
    None,
    CycleNext,
    CyclePrev,
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
