//! Filter Popup Overlay Module
//!
//! Provides a modal filter builder that can be shown over resolution flows
//! via Ctrl+F. Supports filtering by file type, sample rate, bitrate, and
//! duration using the same condition types as the extended tag search.

mod state;
mod render;

pub use state::{FilterCondition, FilterPopupState, FilterPopupAction};
pub use render::render;
