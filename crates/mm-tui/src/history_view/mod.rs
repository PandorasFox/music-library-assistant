//! History View Module
//!
//! Re-exports data and interaction types from mm-ui, plus the TUI-specific renderer.

pub(crate) mod render;

/// Re-export all data types, interaction state, and bundled ViewState from mm-ui.
pub use mm_ui::view_state::lateral::history::{
    ConflictDisposition, ConflictItem, ConflictResolutionState, EditDetailAction, EditDetailEntry,
    EditDetailState, HistoryAction, HistoryInteraction, HistoryPhase, HistoryViewData,
    HistoryViewState, JettisonAllState, JettisonSessionState, ReversalItem, SessionAction,
    SessionListEntry,
};
