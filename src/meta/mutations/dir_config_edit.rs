//! Dir Config Edit Mutation
//!
//! Writes edited source directory config to dirs.kdl.
//! Replaces a single SourceDir entry by matching on path.

use std::path::PathBuf;

use super::traits::{MutationContext, MutationExecutor};
use crate::config::SourceDir;
use crate::meta::computations::Computation;
use crate::meta::mutations::types::{MutationResult, SignalClearScope, SignalToClear};
use crate::meta::recomputation::RecomputationScope;

// Re-export struct definitions from mm-meta
pub use mm_meta::mutations::dir_config_edit::{
    ApplyBatchDirConfigEditsMutation, ApplyDirConfigEditMutation, DirConfigEditEntry,
};

impl MutationExecutor for ApplyDirConfigEditMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::Config
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = (|| -> anyhow::Result<()> {
            let config_dir = crate::config::get_config_dir()?;
            let dirs_path = config_dir.join("dirs.kdl");

            let mut dirs = if dirs_path.exists() {
                let content = std::fs::read_to_string(&dirs_path)?;
                crate::config::parse_dirs_kdl(&content)?
            } else {
                Vec::new()
            };

            let mut found = false;
            for dir in &mut dirs {
                if dir.path == self.source_path {
                    *dir = self.new_dir.clone();
                    found = true;
                    break;
                }
            }
            if !found {
                dirs.push(self.new_dir.clone());
            }

            crate::config::write_dirs_to_disk(&dirs)?;
            crate::logging::log_general(format!(
                "[DIR CONFIG] {} config for {:?}",
                if found { "Updated" } else { "Created" },
                self.source_path
            ));
            Ok(())
        })();
        MutationResult::from_unit_result(
            super::Mutation::ApplyDirConfigEdit(Box::new(self.clone())), result, start,
        )
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn additional_computations(&self) -> Vec<Computation> {
        Vec::new()
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        Vec::new()
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn recomputation_scope(&self) -> RecomputationScope {
        dir_config_recomputation_scope(&self.old_dir, &self.new_dir)
    }
}

// ============================================================================
// Batch Dir Config Edit Mutation
// ============================================================================

impl MutationExecutor for ApplyBatchDirConfigEditsMutation {
    fn origin(&self) -> super::MutationOrigin {
        super::MutationOrigin::Staged
    }
    fn execution_stage(&self) -> super::MutationExecutionStage {
        super::MutationExecutionStage::Config
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let result = (|| -> anyhow::Result<()> {
            let config_dir = crate::config::get_config_dir()?;
            let dirs_path = config_dir.join("dirs.kdl");

            let mut dirs = if dirs_path.exists() {
                let content = std::fs::read_to_string(&dirs_path)?;
                crate::config::parse_dirs_kdl(&content)?
            } else {
                Vec::new()
            };

            for entry in &self.edits {
                let mut found = false;
                for dir in &mut dirs {
                    if dir.path == entry.source_path {
                        *dir = entry.new_dir.clone();
                        found = true;
                        break;
                    }
                }
                if !found {
                    dirs.push(entry.new_dir.clone());
                }
            }

            crate::config::write_dirs_to_disk(&dirs)?;
            crate::logging::log_general(format!(
                "[DIR CONFIG] Batch updated {} dir configs",
                self.edits.len()
            ));
            Ok(())
        })();
        MutationResult::from_unit_result(
            super::Mutation::ApplyBatchDirConfigEdits(Box::new(self.clone())), result, start,
        )
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn additional_computations(&self) -> Vec<Computation> {
        Vec::new()
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        Vec::new()
    }

    fn paths_for_signal_updates(&self) -> Vec<PathBuf> {
        Vec::new()
    }

    fn recomputation_scope(&self) -> RecomputationScope {
        let mut scope = RecomputationScope::EMPTY;
        for entry in &self.edits {
            scope |= dir_config_recomputation_scope(&entry.old_dir, &entry.new_dir);
        }
        scope
    }
}

/// Compute minimal recomputation scope by diffing which SourceDir fields changed.
fn dir_config_recomputation_scope(old: &SourceDir, new: &SourceDir) -> RecomputationScope {
    let mut scope = RecomputationScope::EMPTY;

    // DEPLOY: which libraries this source deploys to
    if old.libraries != new.libraries {
        scope |= RecomputationScope::DEPLOY;
    }

    // FILES: duplicate detection behavior, fingerprinting
    if old.can_stash_dupes != new.can_stash_dupes
        || old.interior_dupes != new.interior_dupes
        || old.enable_acoustid != new.enable_acoustid
    {
        scope |= RecomputationScope::FILES;
    }

    // TAGS: path-tag schema analysis
    let old_schema = old.path_schema.as_ref().map(|s| s.template.as_str());
    let new_schema = new.path_schema.as_ref().map(|s| s.template.as_str());
    if old_schema != new_schema {
        scope |= RecomputationScope::TAGS;
    }

    scope
}
