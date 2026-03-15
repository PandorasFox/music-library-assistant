//! View Stack — push/pop interface for suspending and restoring views.
//!
//! Views are suspended as Routes. When popping, the Route is navigated to,
//! which re-queries fresh data and constructs the view from scratch.
//!
//! Hard navigation (Confirm, Discard, escape-to-health) clears the entire
//! stack so no stale routes leak.

use super::App;
use crate::{transaction_review, ActiveView};

/// Describes which modal we're switching to. Each variant carries an existing
/// state struct that becomes the new `ActiveView`.
pub(crate) enum SuspendTarget {
    TransactionReview(transaction_review::TransactionReviewState),
}

impl SuspendTarget {
    fn into_active_view(self) -> ActiveView {
        match self {
            Self::TransactionReview(s) => ActiveView::TransactionReview(s),
        }
    }
}

impl App {
    /// Push the current view's route onto the stack without switching.
    /// The caller is responsible for setting `self.view` afterwards.
    ///
    /// If the current view has no route (shouldn't happen for normal flows),
    /// nothing is pushed.
    pub(crate) fn push_current_view(&mut self) {
        if let Some(route) = self.view.to_route() {
            self.view_stack.push(route);
        }
    }

    /// Push the current view's route and switch to the target modal.
    pub(crate) fn push_and_switch(&mut self, target: SuspendTarget) {
        self.push_current_view();
        self.view = target.into_active_view();
        self.sync_route();
    }

    /// Pop the most recent route and navigate to it.
    /// Returns false if the stack was empty (caller should navigate to health).
    pub(crate) fn pop_and_restore(&mut self) -> bool {
        match self.view_stack.pop() {
            Some(route) => {
                self.navigate_to(route);
                true
            }
            None => false,
        }
    }

    /// Clear all suspended routes (hard navigation: commit, discard, health).
    pub(crate) fn clear_view_stack(&mut self) {
        self.view_stack.clear();
    }
}
