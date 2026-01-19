//! Startup Flow Modals
//!
//! This module contains the UI modals for the startup flow:
//! - First-time setup (database creation)
//! - Intake confirmation (index unindexed files)
//!
//! These modals run before the main app loop and handle
//! database initialization and initial corpus indexing.

mod first_time_setup;
pub mod intake_confirmation;

pub use first_time_setup::handle_first_time_setup;
pub use intake_confirmation::{IntakeConfirmationState, IntakeConfirmationAction};
