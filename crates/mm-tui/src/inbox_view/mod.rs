//! Inbox View Module
//!
//! Aggregate signal overview for inbox files, similar to the Insights view
//! but scoped to inbox-specific signals. Shows bucket entries with counts
//! rather than individual files.
//!
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.

mod render;

pub use render::render_inbox_view;

// All data + interaction types live in mm-ui.
pub use mm_ui::view_state::lateral::inbox::{
    InboxBucketEntry, InboxInteraction, InboxViewData, InboxViewState,
};
pub use mm_ui::domain_types::InboxInsightAction;
