//! Insights View Module
//!
//! Re-exports data and interaction types from mm-ui, plus the TUI-specific renderer.

mod render;

pub use render::render_insights_view;

/// Re-export all data types and interaction state from mm-ui.
pub use mm_ui::view_state::lateral::health::{
    BucketEntry, CachedBucketEntries, HealthAction, HealthInteraction, InsightListItem,
    InsightType, InsightsViewData, InsightsViewState,
};

pub use mm_ui::domain_types::InsightAction;
