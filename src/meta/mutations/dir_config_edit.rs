//! Dir Config Edit Mutation
//!
//! Writes edited source directory config to dirs.kdl.
//! Replaces a single SourceDir entry by matching on path.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::SourceDir;
use crate::meta::computations::Computation;
use crate::meta::mutations::types::{DiffEntry, MutationResult, SignalClearScope, SignalToClear};
use crate::meta::recomputation::RecomputationScope;
use super::traits::{MutationContext, MutationExecutor};

/// Mutation that applies a single source directory config edit to dirs.kdl.
///
/// Carries the full resulting `Config` so the Witch can update `SharedConfig`
/// in-memory after successful execution (same pattern as `ApplyConfigEdits`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyDirConfigEditMutation {
    /// Which source dir was edited (relative path within corpus).
    pub source_path: PathBuf,
    /// The config as it was before editing (for diffing).
    pub old_dir: SourceDir,
    /// The new config with edits applied.
    pub new_dir: SourceDir,
    /// Full config with the dir edit applied, for SharedConfig update.
    pub new_config: crate::config::Config,
}

impl PartialEq for ApplyDirConfigEditMutation {
    fn eq(&self, other: &Self) -> bool {
        self.source_path == other.source_path
    }
}

impl MutationExecutor for ApplyDirConfigEditMutation {
    fn label(&self) -> &'static str {
        "Dir config update"
    }
    fn staging(&self) -> super::traits::MutationStaging { super::traits::MutationStaging::Staged(super::traits::MutationExecutionStage::Config) }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        let result = (|| -> anyhow::Result<bool> {
            let config_dir = crate::config::get_config_dir()?;
            let dirs_path = config_dir.join("dirs.kdl");

            // Load current dirs
            let mut dirs = if dirs_path.exists() {
                let content = std::fs::read_to_string(&dirs_path)?;
                crate::config::parse_dirs_kdl(&content)?
            } else {
                Vec::new()
            };

            // Find and replace the matching dir, or insert new
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
            Ok(found)
        })();

        match result {
            Ok(was_update) => {
                crate::logging::log_general(format!(
                    "[DIR CONFIG] {} config for {:?}",
                    if was_update { "Updated" } else { "Created" },
                    self.source_path
                ));
                MutationResult {
                    _mutation: super::Mutation::ApplyDirConfigEdit(self.clone()),
                    success: true,
                    error: None,
                    _duration_ms: 0,
                    spawn_mutations: Vec::new(),
                    pending_signals: Vec::new(),
                    discovered_inodes: Vec::new(),
                }
            }
            Err(e) => {
                crate::logging::log_error(format!(
                    "[DIR CONFIG] Config write failed: {:#}",
                    e
                ));
                MutationResult {
                    _mutation: super::Mutation::ApplyDirConfigEdit(self.clone()),
                    success: false,
                    error: Some(format!("Dir config write failed: {:#}", e)),
                    _duration_ms: 0,
                    spawn_mutations: Vec::new(),
                    pending_signals: Vec::new(),
                    discovered_inodes: Vec::new(),
                }
            }
        }
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
                old.can_stash_dupes,
                new.can_stash_dupes,
            ));
        }
        if old.interior_dupes != new.interior_dupes {
            diffs.push(DiffEntry::new(
                "Interior dupes",
                old.interior_dupes,
                new.interior_dupes,
            ));
        }
        let old_schema = old.path_schema.as_ref().map(|s| s.template.as_str()).unwrap_or("(none)");
        let new_schema = new.path_schema.as_ref().map(|s| s.template.as_str()).unwrap_or("(none)");
        if old_schema != new_schema {
            diffs.push(DiffEntry::new(
                "Path schema",
                old_schema,
                new_schema,
            ));
        }
        if old.enable_acoustid != new.enable_acoustid {
            diffs.push(DiffEntry::new(
                "Enable AcoustID",
                old.enable_acoustid,
                new.enable_acoustid,
            ));
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
