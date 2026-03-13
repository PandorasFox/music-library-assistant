//! Jettison edit history mutation.
//!
//! Deletes tag edit history records from the database after the TUI has
//! already exported them to a flat log file. Two modes: single-session
//! and all-sessions.

use serde::{Deserialize, Serialize};

/// Jettison (delete) tag edit history from the database.
///
/// The export-to-flat-file step happens in the TUI *before* this mutation
/// is queued, so this is a pure DB delete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JettisonEditHistoryMutation {
    /// `None` = jettison all sessions, `Some(id)` = jettison single session.
    pub session_id: Option<String>,
    /// Timestamp captured at decision time (ISO 8601 string), used for the
    /// mutation's own record-keeping.
    pub timestamp: String,
}
