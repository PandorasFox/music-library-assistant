//! Health/Insights view interaction state.
//!
//! The health view displays computed insights over corpus signals.
//! Interaction is a single `StandardListState` with busy-blocking on Confirm.

use crate::input::InputAction;
use crate::route::HealthRoute;
use crate::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::view_state::ViewCore;
use crate::wizard::WizardItem;

/// Interaction state for the health/insights view.
pub struct HealthInteraction {
    pub list: StandardListState,
}

/// Action produced by the health view's input handling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HealthAction {
    /// Launch the resolution modal for the selected insight.
    Launch,
}

/// Data context needed for input handling.
pub struct HealthInputCtx<'a, T> {
    /// The flat list items for StandardList navigation.
    pub items: &'a [T],
    /// Whether the Witch has operations in-flight (blocks Confirm).
    pub busy: bool,
}

impl ViewCore for HealthInteraction {
    type Route = HealthRoute;
    type Action = HealthAction;
    type Data = (); // uses generic handle_input_with instead

    fn from_route(route: &HealthRoute) -> Self {
        let mut list = StandardListState::new(StandardListConfig::default());
        if let Some(cursor) = route.cursor {
            list.cursor = cursor;
        }
        Self { list }
    }

    fn to_route(&self) -> HealthRoute {
        HealthRoute {
            cursor: if self.list.cursor > 0 {
                Some(self.list.cursor)
            } else {
                None
            },
        }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<HealthAction> {
        // Use handle_input_with for data-dependent input handling.
        None
    }
}

impl HealthInteraction {
    /// Create a new default interaction state.
    pub fn new() -> Self {
        Self {
            list: StandardListState::new(StandardListConfig::default()),
        }
    }

    /// Handle input with typed list items and busy state.
    ///
    /// This is the primary input handler — `ViewCore::handle_input` exists
    /// for trait conformance but delegates here with proper context.
    pub fn handle_input_with<T: WizardItem + ListEntry>(
        &mut self,
        action: &InputAction,
        ctx: &HealthInputCtx<'_, T>,
    ) -> Option<HealthAction> {
        let result = self.list.handle_input(action, ctx.items);

        match result {
            ListInputResult::Confirm(_) => {
                if ctx.busy {
                    None
                } else {
                    Some(HealthAction::Launch)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_route_default() {
        let interaction = HealthInteraction::from_route(&HealthRoute::default());
        assert_eq!(interaction.list.cursor, 0);
    }

    #[test]
    fn from_route_with_cursor() {
        let interaction = HealthInteraction::from_route(&HealthRoute { cursor: Some(5) });
        assert_eq!(interaction.list.cursor, 5);
    }

    #[test]
    fn to_route_zero_cursor_is_none() {
        let interaction = HealthInteraction::from_route(&HealthRoute::default());
        let route = interaction.to_route();
        assert_eq!(route.cursor, None);
    }

    #[test]
    fn to_route_nonzero_cursor() {
        let mut interaction = HealthInteraction::from_route(&HealthRoute::default());
        interaction.list.cursor = 3;
        let route = interaction.to_route();
        assert_eq!(route.cursor, Some(3));
    }

    #[test]
    fn round_trip_via_route() {
        let mut interaction = HealthInteraction::from_route(&HealthRoute { cursor: Some(7) });
        interaction.list.cursor = 7;
        let route = interaction.to_route();
        let restored = HealthInteraction::from_route(&route);
        assert_eq!(restored.list.cursor, 7);
    }
}
