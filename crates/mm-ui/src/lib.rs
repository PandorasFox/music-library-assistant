//! mm-ui: Shared UI logic for MM backends (TUI, web).
//!
//! This crate contains backend-agnostic widget state machines, input handling,
//! layout structures, and content types. Both `mm-tui` (ratatui terminal) and
//! `mm-web` (HTML + WASM) consume this crate for shared logic.

pub mod click_targets;
pub mod html;
pub mod geometry;
pub mod helpers;
pub mod input;
pub mod lateral_view;
pub mod modal_buttons;
pub mod modal_frame;
pub mod protocol_binding;
pub mod resolution_state;
pub mod rich_text;
pub mod route;
pub mod standard_list;
pub mod text_input;
pub mod wizard;
pub mod wizard_pane;
