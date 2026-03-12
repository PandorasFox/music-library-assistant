//! Database Vacuum UI (historical)
//!
//! Vacuum is now auto-run by the Witch during startup when the freelist
//! ratio exceeds the configured threshold. The UI observes WitchStartupState
//! transitions — no prompt dialog needed.
//! See startup/mod.rs::render_startup_maintenance() for the progress display.
