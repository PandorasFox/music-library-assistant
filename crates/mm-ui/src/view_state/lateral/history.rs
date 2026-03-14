//! History view interaction state.
//!
//! The history view shows tag edit sessions and allows reversal of edits.
//! Interaction owns the session-level list cursor; detail-level and conflict
//! navigation live on the data side because they're created/destroyed as data loads.

use crate::input::InputAction;
use crate::route::HistoryRoute;
use crate::standard_list::{StandardListConfig, StandardListState};
use crate::view_state::ViewCore;

/// Interaction state for the history view.
pub struct HistoryInteraction {
    pub session_list: StandardListState,
}

impl ViewCore for HistoryInteraction {
    type Route = HistoryRoute;
    type Action = ();
    type Data = ();

    fn from_route(_route: &HistoryRoute) -> Self {
        Self {
            session_list: StandardListState::new(StandardListConfig::default()),
        }
    }

    fn to_route(&self) -> HistoryRoute {
        HistoryRoute { session: None }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<()> {
        None
    }
}

impl HistoryInteraction {
    pub fn new() -> Self {
        Self {
            session_list: StandardListState::new(StandardListConfig::default()),
        }
    }
}
