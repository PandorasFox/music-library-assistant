//! View Stack — push/pop interface for suspending and restoring views.
//!
//! When a view needs to open a child modal (e.g., TransactionReview on top of
//! a resolution modal), it pushes the current view onto the stack and switches
//! to the child. When the child completes (Cancel), it pops the stack and
//! restores the parent.
//!
//! Hard navigation (Confirm, Discard, escape-to-health) clears the entire
//! stack so no stale views leak.

use super::App;
use crate::{
    active_view::SuspendedView, insights_view, transaction_review,
    ActiveView,
};

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
    /// Push the current view onto the stack without switching.
    /// The caller is responsible for setting `self.view` afterwards.
    pub(crate) fn push_current_view(&mut self) {
        let suspended = self.suspend_current_view();
        self.view_stack.push(suspended);
    }

    /// Suspend the current view and switch to the target modal.
    pub(crate) fn push_and_switch(&mut self, target: SuspendTarget) {
        let suspended = self.suspend_current_view();
        self.view_stack.push(suspended);
        self.view = target.into_active_view();
    }

    /// Pop the most recent suspended view and restore it.
    /// Returns false if the stack was empty (caller should navigate to health).
    pub(crate) fn pop_and_restore(&mut self) -> bool {
        match self.view_stack.pop() {
            Some(suspended) => self.restore_suspended_view(suspended),
            None => false,
        }
    }

    /// Clear all suspended views (hard navigation: commit, discard, health).
    pub(crate) fn clear_view_stack(&mut self) {
        self.view_stack.clear();
    }

    /// Take the current view and wrap it as a SuspendedView for later restoration.
    ///
    /// V3 views restore directly (all data is already in the view).
    fn suspend_current_view(&mut self) -> SuspendedView {
        let old_view = std::mem::replace(
            &mut self.view,
            ActiveView::Insights {
                data: insights_view::InsightsViewData::new(),
                interaction: insights_view::HealthInteraction::new(),
            },
        );
        SuspendedView::Direct(old_view)
    }

    /// Restore a suspended view.
    /// Returns true if restoration succeeded, false if it failed (caller
    /// should navigate to health).
    fn restore_suspended_view(&mut self, suspended: SuspendedView) -> bool {
        match suspended {
            SuspendedView::Direct(view) => {
                self.view = view;
                true
            }
        }
    }
}
