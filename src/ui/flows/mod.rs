//! UI Flow Handlers Module
//!
//! This module organizes flow-specific handlers that manage complex UI workflows.
//! Flow handlers coordinate between user input, background operations, and state transitions.
//!
//! ## Architecture
//!
//! Each flow handler module provides:
//! - Entry point functions (e.g., `start_*`)
//! - Key/action handlers (e.g., `handle_*_action`)
//! - State transitions back to the main App
//!
//! ## Flow Types
//!
//! - **Tag Editor**: Multi-track metadata editing
//! - **Sleuthing**: Directory browser → dedup flow pipeline
//! - **Background**: Scan, report generation, and other background tasks
//!
//! ## Handler Patterns
//!
//! Flow handlers follow these patterns:
//!
//! 1. Entry functions receive `&mut App` and set up the flow state
//! 2. Action handlers process user input and return next state
//! 3. Completion transitions the App back to MainMenu or another flow
//!
//! ## Future Migration
//!
//! Currently, flow handlers remain in `ui/mod.rs`. As they're extracted,
//! they'll be moved to individual modules here:
//! - `tag_editor.rs` - Tag editor flow handlers
//! - `sleuthing.rs` - Directory browser to dedup pipeline
//! - `background.rs` - Background task handlers

// Placeholder for future extracted flow modules
// pub mod background;
// pub mod sleuthing;
// pub mod tag_editor;
