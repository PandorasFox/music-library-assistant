//! Jettison edit history mutation executors.
//!
//! Two-phase chain:
//! 1. ExportEditHistory (DiskFlush) — queries rows, writes audit log file
//! 2. ClearEditHistory (DB, chain-emitted) — deletes the rows from the database
//!
//! If the export fails, no chain-emission occurs and the DB is untouched.

use std::io::Write;

use anyhow::Result;

use crate::db::write_thread;
use crate::meta::recomputation::RecomputationScope;

use super::traits::{MutationContext, MutationExecutor};
use super::types::{Mutation, MutationResult, SignalClearScope};

pub use mm_meta::mutations::jettison::{ClearEditHistoryMutation, ExportEditHistoryMutation};

// ============================================================================
// ExportEditHistory — Phase 1: write audit log, chain-emit clear
// ============================================================================

impl MutationExecutor for ExportEditHistoryMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }

    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DiskFlush
    }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();

        // Query the rows to export via read-only DB
        let rows = match &self.session_id {
            Some(sid) => ctx.read_db.get_session_edit_history(sid).unwrap_or_default(),
            None => ctx.read_db.get_all_edit_history().unwrap_or_default(),
        };

        if rows.is_empty() {
            crate::logging::log_general(
                "[MUTATION] ExportEditHistory: no rows to export".to_string(),
            );
            return MutationResult::from_unit_result(
                Mutation::ExportEditHistory(self.clone()),
                Ok(()),
                start,
            );
        }

        // Write export log file
        match export_to_log(&rows) {
            Ok(path) => {
                crate::logging::log_general(format!(
                    "[MUTATION] ExportEditHistory: exported {} rows to {}",
                    rows.len(),
                    path,
                ));

                // Chain-emit the DB clear
                let clear = ctx.witness.spawn_mutation(Mutation::ClearEditHistory(
                    ClearEditHistoryMutation {
                        session_id: self.session_id.clone(),
                    },
                ));

                MutationResult {
                    _mutation: Mutation::ExportEditHistory(self.clone()),
                    success: true,
                    error: None,
                    _duration_ms: start.elapsed().as_millis() as u64,
                    spawn_mutations: vec![clear],
                    pending_signals: Vec::new(),
                    discovered_inodes: Vec::new(),
                }
            }
            Err(e) => MutationResult::from_unit_result(
                Mutation::ExportEditHistory(self.clone()),
                Err(anyhow::anyhow!("{}", e)),
                start,
            ),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::EMPTY
    }
}

// ============================================================================
// ClearEditHistory — Phase 2: delete rows from DB (chain-emitted)
// ============================================================================

impl MutationExecutor for ClearEditHistoryMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::ChainEmitted
    }

    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DB
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_clear(&self.session_id);
        MutationResult::from_unit_result(
            Mutation::ClearEditHistory(self.clone()),
            result,
            start,
        )
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn recomputation_scope(&self) -> RecomputationScope {
        RecomputationScope::EMPTY
    }
}

// ============================================================================
// Implementation helpers
// ============================================================================

/// Clear edit history rows via write thread.
fn execute_clear(session_id: &Option<String>) -> Result<()> {
    let sender = write_thread::require_sender()?;

    match session_id {
        Some(sid) => {
            sender.clear_tag_edit_history_session(sid);
            crate::logging::log_general(format!(
                "[MUTATION] ClearEditHistory: session {}",
                sid
            ));
        }
        None => {
            sender.clear_tag_edit_history();
            crate::logging::log_general(
                "[MUTATION] ClearEditHistory: all sessions".to_string(),
            );
        }
    }

    Ok(())
}

/// Export edit history rows to a tab-separated log file.
///
/// Returns the path to the written file on success.
fn export_to_log(rows: &[mm_meta::views::EditHistoryExportRow]) -> Result<String, String> {
    let logs_dir =
        mm_utils::paths::get_logs_dir().map_err(|e| format!("Cannot resolve logs dir: {}", e))?;

    let now = chrono::Local::now();
    let filename = format!(
        "tag_edit_history_export_{}.log",
        now.format("%Y-%m-%d_%H%M%S")
    );
    let path = logs_dir.join(&filename);

    let mut file = std::fs::File::create(&path)
        .map_err(|e| format!("Cannot create {}: {}", path.display(), e))?;

    // Header
    writeln!(
        file,
        "id\tinode\tfield_name\told_value\tnew_value\tedited_at\tsession_id"
    )
    .map_err(|e| format!("Write error: {}", e))?;

    // Data rows
    for row in rows {
        writeln!(
            file,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.id,
            row.inode,
            row.field_name,
            row.old_value.as_deref().unwrap_or(""),
            row.new_value.as_deref().unwrap_or(""),
            row.edited_at,
            row.session_id,
        )
        .map_err(|e| format!("Write error: {}", e))?;
    }

    Ok(path.to_string_lossy().into_owned())
}
