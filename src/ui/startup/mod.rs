//! Startup Modals
//!
//! This module contains the UI modals for the startup:
//! - First-time setup (database creation)
//! - Database migrations (schema upgrades)
//! - Intake confirmation (index unindexed files)
//!
//! These modals run before the main app loop and handle
//! database initialization and initial corpus indexing.
//!
//! Note: Progress screen (startup eyeballing, content analysis) is now
//! handled by the unified `progress_screen` module.

mod first_time_setup;
pub mod intake_confirmation;
mod migrations;
mod vacuum;

pub use first_time_setup::handle_first_time_setup;
pub use intake_confirmation::{IntakeConfirmationState, IntakeConfirmationAction};
pub use migrations::run_migrations;
pub use vacuum::check_and_prompt_vacuum;
