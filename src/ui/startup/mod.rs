//! Startup Modals
//!
//! This module contains the UI modals for the startup:
//! - First-time setup (database creation)
//! - Database migrations (schema upgrades)
//! - Database vacuum (compaction prompt)
//! - Intake confirmation (index unindexed files)
//!
//! Migrations and vacuum are ActiveView variants driven by the main event loop.
//! First-time setup still runs as a pre-loop modal (it creates the DB).
//!
//! Note: Progress screen (startup eyeballing, content analysis) is now
//! handled by the unified `progress_screen` module.

pub(crate) mod first_time_setup;
pub mod intake_confirmation;
pub mod migrations;
pub mod vacuum;

pub use first_time_setup::handle_first_time_setup;
pub use first_time_setup::run_first_time_setup;
pub use intake_confirmation::{IntakeConfirmationState, IntakeConfirmationAction};
