//! External Matches lateral view — browse AcoustID matches by confidence tier.
//!
//! All data + interaction types live in mm-ui.
//! This module keeps the TUI-specific renderer.

pub mod render;

// All data + interaction + action types live in mm-ui.
pub use mm_ui::view_state::lateral::external_matches::{
    ExternalMatchListItem, ExternalMatchesAction, ExternalMatchesInteraction,
    ExternalMatchesViewData, ExternalMatchesViewState, NavigableEntry,
};
