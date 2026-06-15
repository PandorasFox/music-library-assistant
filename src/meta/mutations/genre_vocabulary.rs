//! Genre Vocabulary Edit Execution
//!
//! Applies a batch of operator-curated vocabulary edits (canonical names,
//! aliases, implications, merges) on the DB write thread, then triggers
//! `ImportGenresFromTags` to re-resolve the corpus against the updated
//! alias graph so existing `unresolved_genre_observations` rows can land
//! in the ledger.

use crate::db::write_thread;
use crate::meta::computations::{
    analysis::Computation as AnalysisComputation, Computation,
};
use crate::meta::mutations::types::{
    Mutation, MutationResult, SignalClearScope, SignalToClear,
};
use crate::meta::recomputation::RecomputationScope;

use super::traits::{MutationContext, MutationExecutor};

// Re-export struct definition from mm-meta.
pub use mm_meta::mutations::genre_vocabulary::{
    EditGenreVocabularyMutation, GenreVocabularyOp,
};

impl MutationExecutor for EditGenreVocabularyMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }

    fn execution_stage(&self) -> super::MutationExecutionStage {
        // DB stage: writes go through the db_thread and don't touch disk.
        super::MutationExecutionStage::DB
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = apply_vocabulary_edits(&self.ops);

        if result.is_ok() {
            crate::logging::log_general(format!(
                "[VOCAB] EditGenreVocabulary applied: {} ops",
                self.ops.len()
            ));
        }

        MutationResult::from_unit_result(
            Mutation::EditGenreVocabulary(self.clone()),
            result,
            start,
        )
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        // Vocabulary edits don't invalidate per-inode signals. The ledger
        // re-population happens via the follow-up computation below.
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn additional_computations(&self) -> Vec<Computation> {
        // Re-resolve the corpus against the updated alias graph so newly-mapped
        // aliases pick up previously-unresolved observations. Idempotent at
        // the ledger level via the (inode, genre_id, source, kind) PK.
        vec![Computation::Analysis(AnalysisComputation::ImportGenresFromTags)]
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        Vec::new()
    }

    fn paths_for_signal_updates(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    fn recomputation_scope(&self) -> RecomputationScope {
        // No per-inode dirty tracking — `additional_computations()` queues
        // the corpus-wide re-resolution directly.
        RecomputationScope::EMPTY
    }
}

/// Send the ops through the db_thread and block on completion.
fn apply_vocabulary_edits(ops: &[GenreVocabularyOp]) -> anyhow::Result<()> {
    let sender = write_thread::require_sender()?;
    sender.apply_genre_vocabulary_edits_blocking(ops.to_vec())
}
