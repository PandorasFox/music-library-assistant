//! External Matches view interaction state.
//!
//! Browse AcoustID matches by confidence tier. The interaction is a
//! single StandardListState plus an animation tick counter.

use crate::input::InputAction;
use crate::route::ExternalMatchesRoute;
use crate::standard_list::{ListEntry, StandardListConfig, StandardListState};
use crate::view_state::ViewCore;

/// Interaction state for the external matches view.
pub struct ExternalMatchesInteraction {
    pub list: StandardListState,
    /// Animation tick counter (incremented each UI tick while fetch is active).
    pub tick_count: u32,
}

impl ViewCore for ExternalMatchesInteraction {
    type Route = ExternalMatchesRoute;
    type Action = (); // input handling stays in mm-tui for this view
    type Data = ();

    fn from_route(route: &ExternalMatchesRoute) -> Self {
        let mut list = StandardListState::new(StandardListConfig::default());
        if let Some(cursor) = route.cursor {
            list.cursor = cursor;
        }
        Self { list, tick_count: 0 }
    }

    fn to_route(&self) -> ExternalMatchesRoute {
        ExternalMatchesRoute {
            cursor: if self.list.cursor > 0 {
                Some(self.list.cursor)
            } else {
                None
            },
        }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<()> {
        None
    }
}

impl ExternalMatchesInteraction {
    pub fn new() -> Self {
        Self {
            list: StandardListState::new(StandardListConfig::default()),
            tick_count: 0,
        }
    }

    /// Clamp cursor position to valid range after data refresh.
    pub fn clamp_to_data<T: ListEntry>(&mut self, items: &[T]) {
        self.list.clamp_cursor(items);
    }
}
