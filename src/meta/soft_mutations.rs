//! Soft mutation execution — library-zone filesystem operations.
//!
//! Each soft mutation variant reuses the same filesystem helpers as the
//! corresponding transaction-staged mutation (HardLink, LibraryMove,
//! StashLeftovers) but bypasses the transaction/decision system entirely.

use std::path::PathBuf;

use mm_meta::soft_mutations::SoftMutation;

use crate::corpus::paths;
use crate::db::types::Zone;
use crate::meta::computations::{derivation, Computation};
use crate::meta::mutations::file_ops;
use crate::meta::mutations::SignalToClear;
use crate::meta::recomputation::RecomputationScope;
use crate::meta::signals::data::*;

// ============================================================================
// Result type
// ============================================================================

/// Result of executing a soft mutation.
pub struct SoftMutationResult {
    pub success: bool,
    pub error: Option<String>,
    /// Inodes discovered at execution time (for stash index cleanup).
    pub discovered_inodes: Vec<i64>,
}

// ============================================================================
// Execution dispatch
// ============================================================================

/// Execute a soft mutation's filesystem operation.
pub fn execute(sm: &SoftMutation, stash_root: Option<&PathBuf>) -> SoftMutationResult {
    match sm {
        SoftMutation::DeployLink { source, destination } => {
            match file_ops::execute_hard_link(source, destination) {
                Ok(()) => SoftMutationResult {
                    success: true,
                    error: None,
                    discovered_inodes: Vec::new(),
                },
                Err(e) => SoftMutationResult {
                    success: false,
                    error: Some(format!("{:#}", e)),
                    discovered_inodes: Vec::new(),
                },
            }
        }

        SoftMutation::DeployMove { source, destination } => {
            match file_ops::execute_library_move_impl(source, destination) {
                Ok(()) => SoftMutationResult {
                    success: true,
                    error: None,
                    discovered_inodes: Vec::new(),
                },
                Err(e) => SoftMutationResult {
                    success: false,
                    error: Some(format!("{:#}", e)),
                    discovered_inodes: Vec::new(),
                },
            }
        }

        SoftMutation::StashLibrary { path } => {
            use std::os::unix::fs::MetadataExt;

            let discovered = std::fs::metadata(path)
                .map(|m| vec![m.ino() as i64])
                .unwrap_or_default();

            match stash_root {
                Some(root) => {
                    match file_ops::execute_move_to_stash(path, "library_leftovers", root) {
                        Ok(()) => SoftMutationResult {
                            success: true,
                            error: None,
                            discovered_inodes: discovered,
                        },
                        Err(e) => SoftMutationResult {
                            success: false,
                            error: Some(format!("{:#}", e)),
                            discovered_inodes: discovered,
                        },
                    }
                }
                None => SoftMutationResult {
                    success: false,
                    error: Some(format!(
                        "Stash directory not configured. File: {}",
                        path.display()
                    )),
                    discovered_inodes: discovered,
                },
            }
        }
    }
}

// ============================================================================
// Post-execution metadata
// ============================================================================

/// Paths that need signal update computations spawned.
pub fn paths_for_signal_updates(sm: &SoftMutation) -> Vec<PathBuf> {
    match sm {
        SoftMutation::DeployLink { source, destination } => {
            vec![source.clone(), destination.clone()]
        }
        SoftMutation::DeployMove { source, destination } => {
            vec![source.clone(), destination.clone()]
        }
        SoftMutation::StashLibrary { .. } => Vec::new(),
    }
}

/// Additional computations to spawn after execution.
pub fn additional_computations(sm: &SoftMutation) -> Vec<Computation> {
    match sm {
        SoftMutation::DeployLink { source, destination } => {
            vec![Computation::Derivation(
                derivation::Computation::UpdateDeploySignals {
                    corpus_path: source.clone(),
                    library_path: destination.clone(),
                },
            )]
        }
        _ => Vec::new(),
    }
}

/// Specific aggregate signals to clear by exact key.
pub fn specific_signals_to_clear(sm: &SoftMutation) -> Vec<SignalToClear> {
    let resolver = paths::get_resolver();
    match sm {
        SoftMutation::DeployMove { source, .. } => {
            if let Some(lib_rel) = resolver.to_zone_relative(source, Zone::Library) {
                let library_path = lib_rel.to_string_lossy();
                if let Some(library_name) = library_path.split('/').next() {
                    let key = LibraryStaleSignal::make_key(library_name, &library_path);
                    return vec![SignalToClear::exact::<LibraryStaleSignal>(key)];
                }
            }
            Vec::new()
        }
        SoftMutation::StashLibrary { path } => {
            if let Some(lib_rel) = resolver.to_zone_relative(path, Zone::Library) {
                let library_path = lib_rel.to_string_lossy();
                if let Some(library_name) = library_path.split('/').next() {
                    let key = LibraryLeftoverSignal::make_key(library_name, &library_path);
                    return vec![SignalToClear::exact::<LibraryLeftoverSignal>(key)];
                }
            }
            Vec::new()
        }
        _ => Vec::new(),
    }
}

/// Recomputation scope indicating which domains this operation dirtied.
pub fn recomputation_scope(sm: &SoftMutation) -> RecomputationScope {
    match sm {
        SoftMutation::DeployLink { .. } => RecomputationScope::DEPLOY,
        SoftMutation::DeployMove { .. } => RecomputationScope::DEPLOY,
        SoftMutation::StashLibrary { .. } => RecomputationScope::FILES | RecomputationScope::DEPLOY,
    }
}

/// Whether this soft mutation should drop discovered inodes from the files table.
pub fn drops_from_index(sm: &SoftMutation) -> bool {
    matches!(sm, SoftMutation::StashLibrary { .. })
}
