//! Inbox view interaction state.
//!
//! The inbox view shows aggregate bucket entries for inbox-scoped signals.
//! Structurally identical to health — a single StandardListState with busy-blocking.

use crate::domain_types::InboxInsightAction;
use crate::input::InputAction;
use crate::route::InboxRoute;
use crate::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::view_state::ViewCore;
use crate::wizard::WizardItem;

/// Interaction state for the inbox view.
pub struct InboxInteraction {
    pub list: StandardListState,
}

/// Data context needed for input handling.
pub struct InboxInputCtx<'a, T> {
    pub items: &'a [T],
    pub busy: bool,
}

impl ViewCore for InboxInteraction {
    type Route = InboxRoute;
    type Action = InboxInsightAction;
    type Data = ();

    fn from_route(route: &InboxRoute) -> Self {
        let mut list = StandardListState::new(StandardListConfig::default());
        if let Some(cursor) = route.cursor {
            list.cursor = cursor;
        }
        Self { list }
    }

    fn to_route(&self) -> InboxRoute {
        InboxRoute {
            cursor: if self.list.cursor > 0 {
                Some(self.list.cursor)
            } else {
                None
            },
        }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<InboxInsightAction> {
        None
    }
}

impl InboxInteraction {
    pub fn new() -> Self {
        Self {
            list: StandardListState::new(StandardListConfig::default()),
        }
    }

    /// Handle input with typed list items and busy state.
    pub fn handle_input_with<T: WizardItem + ListEntry<Action = InboxInsightAction>>(
        &mut self,
        action: &InputAction,
        ctx: &InboxInputCtx<'_, T>,
    ) -> Option<InboxInsightAction> {
        let result = self.list.handle_input(action, ctx.items);

        match result {
            ListInputResult::Confirm(insight_action) => {
                if ctx.busy {
                    None
                } else {
                    Some(insight_action)
                }
            }
            ListInputResult::Consumed
            | ListInputResult::CursorMoved
            | ListInputResult::Toggled
            | ListInputResult::Unhandled => None,
        }
    }

    /// Clamp cursor position to valid range after data refresh.
    pub fn clamp_to_data<T: ListEntry>(&mut self, items: &[T]) {
        self.list.clamp_cursor(items);
    }
}
