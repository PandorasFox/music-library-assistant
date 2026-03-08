//! Filter Popup Overlay Module
//!
//! Provides a modal filter builder that can be shown over resolution modals
//! via Ctrl+/. Supports filtering by file type, sample rate, bitrate, and
//! duration using the same condition types as the extended tag search.

mod render;
mod state;

pub use render::render;
pub use state::{FilterCondition, FilterPopupAction, FilterPopupState};
