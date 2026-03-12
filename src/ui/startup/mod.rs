//! Startup UI
//!
//! This module contains the UI components for startup flows:
//! - First-time setup (directory picker, DB setup dialog) — runs pre-loop in `run_tui()`
//! - Database migrations (schema upgrades)
//! - Database vacuum (compaction prompt)
//! - Intake confirmation (index unindexed files)
//!
//! Migrations and vacuum are ActiveView variants driven by the main event loop.
//! First-time setup runs as a blocking pre-loop step: `run_tui()` checks
//! `WitchStartupState::AwaitingSetup`, runs the setup UI, then sends
//! `CompleteSetup` to the Witch before entering the main event loop.
//!
//! Note: Progress screen (startup eyeballing, content analysis) is now
//! handled by the unified `progress_screen` module.

pub(crate) mod first_time_setup;
pub mod intake_confirmation;
pub mod migrations;
pub mod vacuum;

pub use first_time_setup::handle_db_setup_dialog;
pub use first_time_setup::run_directory_picker;
pub use intake_confirmation::{IntakeConfirmationAction, IntakeConfirmationState, IntakeSource};
