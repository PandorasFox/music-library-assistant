//! Jettison edit history mutations.
//!
//! Two-phase chain: ExportEditHistory (DiskFlush) writes the audit log,
//! then chain-emits ClearEditHistory (DB) to delete the rows. If the
//! export fails, the chain-emission never happens and the DB is untouched.

use serde::{Deserialize, Serialize};

/// Export tag edit history to a log file, then chain-emit DB clear.
///
/// Stage: DiskFlush (filesystem write). Origin: Staged (operator-confirmed).
///
/// During execution:
/// 1. Queries edit history rows via read-only DB
/// 2. Writes them to a timestamped log file
/// 3. On success, chain-emits `ClearEditHistory` to delete the rows
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExportEditHistoryMutation {
    /// `None` = export all sessions, `Some(id)` = export single session.
    pub session_id: Option<String>,
}

impl ExportEditHistoryMutation {
    pub fn diff_entries(&self) -> Vec<super::types::DiffEntry> {
        let scope = match &self.session_id {
            Some(id) => format!("session {}", id),
            None => "all sessions".to_string(),
        };
        vec![super::types::DiffEntry::new(
            "jettison",
            "",
            &format!("export + clear {}", scope),
        )]
    }
}

/// Clear tag edit history rows from the database.
///
/// Stage: DB. Origin: ChainEmitted (only spawned by ExportEditHistory).
///
/// This is the second phase of jettison — the export log has already been
/// written successfully before this mutation is spawned.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClearEditHistoryMutation {
    /// `None` = clear all sessions, `Some(id)` = clear single session.
    pub session_id: Option<String>,
}

impl ClearEditHistoryMutation {
    pub fn diff_entries(&self) -> Vec<super::types::DiffEntry> {
        let scope = match &self.session_id {
            Some(id) => format!("session {}", id),
            None => "all sessions".to_string(),
        };
        vec![super::types::DiffEntry::new(
            "clear edit history",
            &scope,
            "(deleted)",
        )]
    }
}
