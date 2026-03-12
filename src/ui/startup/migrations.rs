//! Schema Update UI (historical)
//!
//! Schema reconciliation is now auto-run by the Witch during startup.
//! The UI observes WitchStartupState transitions — no approval dialog needed.
//! See startup/mod.rs::render_startup_maintenance() for the progress display.
