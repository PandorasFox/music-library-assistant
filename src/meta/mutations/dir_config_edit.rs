//! Dir Config Edit Mutation
//!
//! Writes edited source directory config to dirs.kdl.
//! Replaces a single SourceDir entry by matching on path.

use std::path::PathBuf;

use super::traits::{MutationContext, MutationExecutor};
use crate::config::SourceDir;
use crate::meta::computations::Computation;
use crate::meta::mutations::types::{DiffEntry, MutationResult, SignalClearScope, SignalToClear};
use crate::meta::recomputation::RecomputationScope;

// Re-export struct definitions from mm-meta
pub use mm_meta::mutations::dir_config_edit::{
    ApplyBatchDirConfigEditsMutation, ApplyDirConfigEditMutation, DirConfigEditEntry,
};

impl MutationExecutor for ApplyDirConfigEditMutation {
    fn label(&self) -> &'static str {
        "Dir config update"
    }
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

    fn diff_entries(&self) -> Vec<DiffEntry> {
        let mut diffs = Vec::new();
        let old = &self.old_dir;
        let new = &self.new_dir;

        if old.libraries != new.libraries {
            diffs.push(DiffEntry::new(
                "Libraries",
                old.libraries.join(", "),
                new.libraries.join(", "),
            ));
        }
        if old.can_stash_dupes != new.can_stash_dupes {
            diffs.push(DiffEntry::new(
                "Can stash dupes",
                format_opt_bool(old.can_stash_dupes),
                format_opt_bool(new.can_stash_dupes),
            ));
        }
        if old.interior_dupes != new.interior_dupes {
            diffs.push(DiffEntry::new(
                "Interior dupes",
                format_opt_bool(old.interior_dupes),
                format_opt_bool(new.interior_dupes),
            ));
        }
        let old_schema = old
            .path_schema
            .as_ref()
            .map(|s| s.template.as_str())
            .unwrap_or("(none)");
        let new_schema = new
            .path_schema
            .as_ref()
            .map(|s| s.template.as_str())
            .unwrap_or("(none)");
        if old_schema != new_schema {
            diffs.push(DiffEntry::new("Path schema", old_schema, new_schema));
        }
        if old.enable_acoustid != new.enable_acoustid {
            diffs.push(DiffEntry::new(
                "Enable AcoustID",
                format_opt_bool(old.enable_acoustid),
                format_opt_bool(new.enable_acoustid),
            ));
        }
        if old.pinned_release != new.pinned_release {
            diffs.push(DiffEntry::new(
                "Pinned release",
                old.pinned_release.as_deref().unwrap_or("(none)"),
                new.pinned_release.as_deref().unwrap_or("(none)"),
            ));
        }

        diffs
    }
}

/// Format an Option<bool> for diff display.
fn format_opt_bool(v: Option<bool>) -> &'static str {
    match v {
        Some(true) => "true",
        Some(false) => "false",
        None => "(inherit)",
    }
}

// ============================================================================
// Batch Dir Config Edit Mutation
// ============================================================================

impl MutationExecutor for ApplyBatchDirConfigEditsMutation {
    fn label(&self) -> &'static str {
        "Batch dir config update"
    }

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

    fn diff_entries(&self) -> Vec<DiffEntry> {
        let mut diffs = Vec::new();
        for entry in &self.edits {
            let old = &entry.old_dir;
            let new = &entry.new_dir;
            let prefix = entry.source_path.display().to_string();

            if old.libraries != new.libraries {
                diffs.push(DiffEntry::new(
                    format!("{}: Libraries", prefix),
                    old.libraries.join(", "),
                    new.libraries.join(", "),
                ));
            }
            if old.can_stash_dupes != new.can_stash_dupes {
                diffs.push(DiffEntry::new(
                    format!("{}: Can stash dupes", prefix),
                    format_opt_bool(old.can_stash_dupes),
                    format_opt_bool(new.can_stash_dupes),
                ));
            }
            if old.interior_dupes != new.interior_dupes {
                diffs.push(DiffEntry::new(
                    format!("{}: Interior dupes", prefix),
                    format_opt_bool(old.interior_dupes),
                    format_opt_bool(new.interior_dupes),
                ));
            }
            let old_schema = old
                .path_schema
                .as_ref()
                .map(|s| s.template.as_str())
                .unwrap_or("(none)");
            let new_schema = new
                .path_schema
                .as_ref()
                .map(|s| s.template.as_str())
                .unwrap_or("(none)");
            if old_schema != new_schema {
                diffs.push(DiffEntry::new(
                    format!("{}: Path schema", prefix),
                    old_schema,
                    new_schema,
                ));
            }
            if old.enable_acoustid != new.enable_acoustid {
                diffs.push(DiffEntry::new(
                    format!("{}: Enable AcoustID", prefix),
                    format_opt_bool(old.enable_acoustid),
                    format_opt_bool(new.enable_acoustid),
                ));
            }
            if old.pinned_release != new.pinned_release {
                diffs.push(DiffEntry::new(
                    format!("{}: Pinned release", prefix),
                    old.pinned_release.as_deref().unwrap_or("(none)"),
                    new.pinned_release.as_deref().unwrap_or("(none)"),
                ));
            }
        }
        diffs
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
