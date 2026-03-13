//! Config Edit Mutation
//!
//! Writes edited config to disk (comment-preserving KDL modification).
//! Returns the new Config in-band via TaskResult.config_update.

use super::traits::{MutationContext, MutationExecutor};
use crate::config::Config;
use crate::meta::computations::Computation;
use crate::meta::mutations::types::{MutationResult, SignalClearScope, SignalToClear};
use crate::meta::recomputation::RecomputationScope;

// Re-export struct definition from mm-meta
pub use mm_meta::mutations::config_edit::ApplyConfigEditsMutation;

impl MutationExecutor for ApplyConfigEditsMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::Config
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = crate::config::write_config_to_disk(
            &self.original_kdl, &self.old_config, &self.new_config,
        );
        if result.is_ok() {
            crate::logging::log_general("[CONFIG] Config written to disk successfully");
        } else {
            crate::logging::log_error(format!("[CONFIG] Config write failed: {:#}", result.as_ref().unwrap_err()));
        }
        MutationResult::from_unit_result(
            super::Mutation::ApplyConfigEdits(Box::new(self.clone())), result, start,
        )
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn additional_computations(&self) -> Vec<Computation> {
        let new_pairs = new_tag_separator_pairs(&self.old_config, &self.new_config);
        if new_pairs.is_empty() {
            return Vec::new();
        }
        vec![Computation::Analysis(
            crate::meta::computations::analysis::Computation::SeedCompoundTagDirtyInodes {
                new_separators: new_pairs,
            },
        )]
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        Vec::new()
    }

    fn paths_for_signal_updates(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    fn recomputation_scope(&self) -> RecomputationScope {
        config_recomputation_scope(&self.old_config, &self.new_config)
    }
}

/// Extract (tag_name, separator) pairs that are new in the updated config.
///
/// Compares old and new tag_splitting.tag_separators, returning pairs where
/// the separator was not present for that tag in the old config.
fn new_tag_separator_pairs(old: &Config, new: &Config) -> Vec<(String, String)> {
    let old_seps = &old.opinions.tag_splitting.tag_separators;
    let new_seps = &new.opinions.tag_splitting.tag_separators;
    let mut pairs = Vec::new();
    for (tag, new_sep_list) in new_seps {
        let old_sep_list = old_seps.get(tag);
        for sep in new_sep_list {
            let is_new = old_sep_list.is_none_or(|old| !old.contains(sep));
            if is_new {
                pairs.push((tag.clone(), sep.clone()));
            }
        }
    }
    pairs
}

/// Determine which domains a config change affects.
///
/// Inspects old vs new config field-by-field to produce a precise scope.
/// Fields that only affect runtime policy (startup, performance, quality resolution)
/// return EMPTY — they don't need content re-analysis.
fn config_recomputation_scope(old: &Config, new: &Config) -> RecomputationScope {
    let o = &old.opinions;
    let n = &new.opinions;
    let mut scope = RecomputationScope::EMPTY;

    // TAGS: fields that affect tag-sensitive computations
    if o.health_detection.required_tags != n.health_detection.required_tags
        || o.health_detection.album_artist_only_required_if_compilation
            != n.health_detection.album_artist_only_required_if_compilation
        || o.health_detection.single_album_suffix != n.health_detection.single_album_suffix
        || o.canonicalization.strip_album_format_suffixes
            != n.canonicalization.strip_album_format_suffixes
        || o.tag_splitting != n.tag_splitting
    {
        scope |= RecomputationScope::TAGS;
    }

    // FILES: fields that affect file/fingerprint/duplicate detection
    if o.duplicate_analysis != n.duplicate_analysis
        || o.lossy_shit_formats_to_flac != n.lossy_shit_formats_to_flac
    {
        scope |= RecomputationScope::FILES;
    }

    // INBOX: fields that affect inbox computations
    if o.inbox_organize != n.inbox_organize {
        scope |= RecomputationScope::INBOX;
    }

    // These fields are runtime policy — no content re-analysis needed:
    // startup.*, performance.*, leave_transactions_open, quality_resolution.*

    scope
}
