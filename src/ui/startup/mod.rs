//! Startup Flow Modals
//!
//! This module contains the UI modals for the startup flow:
//! - First-time setup (database creation)
//! - Database migrations (schema upgrades)
//! - Intake confirmation (index unindexed files)
//! - Content analysis progress (post-intake health signal computation)
//!
//! These modals run before the main app loop and handle
//! database initialization and initial corpus indexing.

pub mod content_analysis;
mod first_time_setup;
pub mod intake_confirmation;
mod migrations;

pub use content_analysis::ContentAnalysisProgress;
pub use first_time_setup::handle_first_time_setup;
pub use intake_confirmation::{IntakeConfirmationState, IntakeConfirmationAction};
pub use migrations::check_and_run_migrations;
