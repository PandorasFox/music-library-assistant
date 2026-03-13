//! Jettison edit history mutation executor.
//!
//! DB-only mutation that deletes tag edit history rows. No filesystem ops,
//! no signals, no recomputation.

use anyhow::Result;

use crate::db::write_thread;
use crate::meta::recomputation::RecomputationScope;

use super::traits::{MutationContext, MutationExecutor};
use super::types::{Mutation, MutationResult, SignalClearScope};

// Re-export struct definition from mm-meta
pub use mm_meta::mutations::jettison::JettisonEditHistoryMutation;

impl MutationExecutor for JettisonEditHistoryMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }

    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::DB
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = execute_jettison(&self.session_id);
        MutationResult::from_unit_result(
            Mutation::JettisonEditHistory(self.clone()),
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

/// Execute jettison: delete edit history rows via write thread.
fn execute_jettison(session_id: &Option<String>) -> Result<()> {
    let sender = write_thread::require_sender()?;

    match session_id {
        Some(sid) => {
            sender.clear_tag_edit_history_session(sid);
            crate::logging::log_general(format!(
                "[MUTATION] JettisonEditHistory: session {}",
                sid
            ));
        }
        None => {
            sender.clear_tag_edit_history();
            crate::logging::log_general(
                "[MUTATION] JettisonEditHistory: all sessions".to_string(),
            );
        }
    }

    Ok(())
}
