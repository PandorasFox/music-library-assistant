//! Deploy-related executors.
//!
//! Deploy conflict detection, deploy health signals, and corpus deploy status.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    TypedSignalWrite, DeployConflictSignal, DeployReadySignal, DeployedHealthySignal,
    LibraryLeftoverSignal, LibraryStaleSignal,
};
use crate::corpus::deploy::compute_deployment_path_with_tags;
use crate::corpus::db::ReadOnlyDb;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Deploy Conflict Detection
// ============================================================================

/// Execute DetectDeployConflicts - bulk detection of deploy path collisions.
pub fn execute_detect_deploy_conflicts(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectDeployConflicts;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Clear all existing DeployConflict signals (routes through db_thread)
    sender.clear_all_of_aggregate_type::<DeployConflictSignal>(witness);

    let healthy_signals = read_only_db
        .get_healthy_file_signals()
        .unwrap_or_default();

    let mut deploy_path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for signal in &healthy_signals {
        let tags = read_only_db.get_corpus_tags(signal.inode).unwrap_or_default();
        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
            .collect();

        let deploy_path = compute_deployment_path_with_tags(&signal.path, &tag_map)
            .to_string_lossy()
            .to_string();

        deploy_path_to_tracks
            .entry(deploy_path)
            .or_default()
            .push(signal.inode);
    }

    let mut conflict_count = 0;
    for (deploy_path, inodes) in deploy_path_to_tracks {
        if inodes.len() > 1 {
            conflict_count += 1;
            sender.write_typed_signal(
                TypedSignalWrite::DeployConflict(DeployConflictSignal {
                    key: deploy_path.clone(),
                    deploy_path: deploy_path.clone(),
                    inodes: inodes.clone(),
                }),
                witness,
            );
        }
    }

    log_general(format!(
        "[COMPUTE] DetectDeployConflicts: {} conflicts among {} healthy files",
        conflict_count,
        healthy_signals.len()
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Deploy Health Signals
// ============================================================================

/// Execute DeriveDeployHealthSignals - derive library health from scan data.
///
/// Reads library file data from files table (source='library') and compares against
/// corpus index to identify leftovers and stale deployments.
pub fn execute_derive_deploy_health_signals(
    read_only_db: &ReadOnlyDb<'_>,
    library_name: &str,
    library_root: &Path,
    corpus_path_prefixes: &[std::path::PathBuf],
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DeriveDeployHealthSignals {
        library_name: library_name.to_string(),
        library_root: library_root.to_path_buf(),
        corpus_path_prefixes: corpus_path_prefixes.to_vec(),
    };

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': starting (root={:?}, prefixes={:?})",
        library_name, library_root, corpus_path_prefixes,
    ));

    // Bulk clear all existing leftover/stale signals for this library before recomputing.
    // This prevents stale signals from persisting when files are removed between runs.
    let leftover_prefix = LibraryLeftoverSignal::key_prefix_for_library(library_name);
    let stale_prefix = LibraryStaleSignal::key_prefix_for_library(library_name);
    sender.clear_aggregate_by_key_prefix::<LibraryLeftoverSignal>(&leftover_prefix, witness);
    sender.clear_aggregate_by_key_prefix::<LibraryStaleSignal>(&stale_prefix, witness);

    // Query library file data from Awakening phase
    let library_scan_entries = match read_only_db.get_library_files(library_name) {
        Ok(entries) => entries,
        Err(e) => {
            log_general(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': failed to get scan data: {}",
                library_name, e
            ));
            return Result::success(computation, start.elapsed().as_millis() as u64, Vec::new());
        }
    };

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': got {} library scan entries from DB",
        library_name, library_scan_entries.len(),
    ));

    // Log first few entries for debugging
    for (i, entry) in library_scan_entries.iter().take(3).enumerate() {
        log_general(format!(
            "[COMPUTE] DeriveDeployHealthSignals '{}': sample lib[{}] path={:?} inode={}",
            library_name, i, entry.file_path, entry.inode,
        ));
    }

    // Convert to (path, inode) tuples for processing
    let library_files: Vec<(std::path::PathBuf, i64)> = library_scan_entries
        .into_iter()
        .map(|entry| (entry.file_path, entry.inode))
        .collect();

    // Get all corpus audio file inodes
    let corpus_inodes = read_only_db.get_all_corpus_inodes().unwrap_or_default();

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': {} corpus inodes loaded",
        library_name, corpus_inodes.len(),
    ));

    let mut healthy_count: usize = 0;
    let mut stale_count: usize = 0;
    let mut leftover_count: usize = 0;
    let mut debug_logged = 0usize;

    for (library_path, library_inode) in &library_files {
        // Log first few inode lookups to trace match/miss behavior
        if debug_logged < 5 {
            let corpus_match = corpus_inodes.get(library_inode);
            log_general(format!(
                "[COMPUTE] DeriveDeployHealthSignals '{}': lib inode {} path={:?} -> corpus={:?}",
                library_name, library_inode, library_path, corpus_match,
            ));
            debug_logged += 1;
        }

        let library_path_display = library_path.display().to_string();

        if let Some(corpus_path) = corpus_inodes.get(library_inode) {
            // Has corpus backing — check if stale
            let is_stale = if let Ok(Some(audio_file)) = read_only_db.get_audio_file_by_path(corpus_path) {
                let inode = audio_file.inode();
                let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
                let tag_map: std::collections::HashMap<String, String> = tags
                    .into_iter()
                    .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                    .collect();

                // Compute expected relative path within the library
                let expected_relative = compute_deployment_path_with_tags(corpus_path, &tag_map);

                // library_path is library-name-prefixed (e.g., "soundtracks/Artist/Album/track.mp3")
                // expected_relative is just "Artist/Album/track.mp3" (no library prefix)
                // Strip the library name prefix for comparison
                let library_path_suffix = library_path
                    .strip_prefix(library_name)
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|_| library_path.clone());

                if library_path_suffix != expected_relative {
                    // Stale: store paths with consistent library prefix for display and mutations
                    let expected_with_prefix = std::path::Path::new(library_name).join(&expected_relative);
                    let stale_key = LibraryStaleSignal::make_key(library_name, &library_path_display);
                    sender.write_typed_signal(
                        TypedSignalWrite::LibraryStale(LibraryStaleSignal {
                            key: stale_key,
                            library_path: library_path_display,
                            expected_path: expected_with_prefix.to_string_lossy().to_string(),
                            corpus_path: corpus_path.clone(),
                            inode,
                        }),
                        witness,
                    );
                    true
                } else {
                    false
                }
            } else {
                false
            };

            if is_stale {
                stale_count += 1;
            } else {
                healthy_count += 1;
            }
        } else {
            // No corpus backing — leftover
            leftover_count += 1;
            let leftover_key = LibraryLeftoverSignal::make_key(library_name, &library_path_display);
            sender.write_typed_signal(
                TypedSignalWrite::LibraryLeftover(LibraryLeftoverSignal {
                    key: leftover_key,
                }),
                witness,
            );
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveDeployHealthSignals '{}': {} files, {} healthy, {} stale, {} leftover",
        library_name,
        library_files.len(),
        healthy_count,
        stale_count,
        leftover_count,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}

// ============================================================================
// Corpus Deploy Status
// ============================================================================

/// Execute DeriveCorpusDeployStatus - derive corpus-side deployment signals.
///
/// For each HealthyFile signal, checks if the file's inode exists in any library
/// (via files table) and emits:
/// - DeployReady: healthy file not in any library
/// - DeployedHealthy: healthy file correctly deployed (in library, not stale)
pub fn execute_derive_corpus_deploy_status(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DeriveCorpusDeployStatus;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Bulk clear all existing deploy status signals before recomputing.
    // This prevents stale signals from prior runs (with outdated metadata)
    // from persisting and inflating counts.
    sender.clear_all_of_corpus_type::<DeployReadySignal>(witness);
    sender.clear_all_of_corpus_type::<DeployedHealthySignal>(witness);

    // Get all HealthyFile signals
    let healthy_signals = read_only_db
        .get_healthy_file_signals()
        .unwrap_or_default();

    // Build inode → library paths map from files table (source='library')
    // This replaces the old stale_signals/stale_inodes approach that had a race
    // condition with DeriveDeployHealthSignals. We compute stale status inline.
    let mut library_inode_to_paths: HashMap<i64, Vec<PathBuf>> = HashMap::new();
    match read_only_db.get_all_library_files() {
        Ok(all_library_files) => {
            log_general(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: loaded {} library files from DB",
                all_library_files.len(),
            ));
            for entry in all_library_files {
                library_inode_to_paths
                    .entry(entry.inode)
                    .or_default()
                    .push(entry.file_path);
            }
        }
        Err(e) => {
            log_general(format!(
                "[COMPUTE] DeriveCorpusDeployStatus: WARNING - get_all_library_files failed: {}",
                e,
            ));
        }
    }

    // Get deploy-configured corpus paths to filter healthy files
    let config = match crate::config::load_config() {
        Ok(c) => c,
        Err(_) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "Failed to load config".to_string(),
            );
        }
    };

    // Phase 1: Pre-compute deploy paths for all configured healthy files.
    // We need to see ALL deploy paths before emitting signals, because files
    // whose deploy path is shared by 2+ corpus files are conflict losers and
    // should NOT be marked DeployReady.
    struct PrecomputedFile {
        inode: i64,
        corpus_path: String,
        deploy_path: String,
    }

    let mut precomputed: Vec<PrecomputedFile> = Vec::new();
    let mut deploy_path_counts: HashMap<String, usize> = HashMap::new();
    let mut skipped_not_configured = 0usize;

    for signal in &healthy_signals {
        let corpus_path_buf = Path::new(&signal.path);

        if !config.is_path_in_source(corpus_path_buf) {
            skipped_not_configured += 1;
            continue;
        }

        let tags = read_only_db.get_corpus_tags(signal.inode).unwrap_or_default();
        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
            .collect();
        let expected_relative = compute_deployment_path_with_tags(&signal.path, &tag_map);
        let deploy_path = expected_relative.to_string_lossy().to_string();

        *deploy_path_counts.entry(deploy_path.clone()).or_insert(0) += 1;

        precomputed.push(PrecomputedFile {
            inode: signal.inode,
            corpus_path: signal.path.clone(),
            deploy_path,
        });
    }

    // Phase 2: Build conflict set — deploy paths claimed by 2+ corpus files.
    let conflict_paths: HashSet<&str> = deploy_path_counts
        .iter()
        .filter(|(_, count)| **count > 1)
        .map(|(path, _)| path.as_str())
        .collect();

    // Phase 3: Emit signals, skipping conflict losers from DeployReady.
    let mut deploy_ready_count = 0usize;
    let mut deployed_healthy_count = 0usize;
    let mut conflict_skipped_count = 0usize;

    for file in &precomputed {
        // Check if this inode is deployed in any library
        if let Some(library_paths) = library_inode_to_paths.get(&file.inode) {
            // File is deployed — check if any library path matches expected
            let matching_path = library_paths.iter().find(|lp| {
                // Library paths are "{library_name}/{relative_path}"
                // Strip library name prefix for comparison with expected deploy path
                let components: Vec<_> = lp.components().collect();
                if components.len() > 1 {
                    let suffix: PathBuf = components[1..].iter().collect();
                    suffix == Path::new(&file.deploy_path)
                } else {
                    false
                }
            });

            if let Some(lib_path) = matching_path {
                // Correctly deployed
                deployed_healthy_count += 1;
                sender.write_typed_signal(
                    TypedSignalWrite::DeployedHealthy(DeployedHealthySignal {
                        inode: file.inode,
                        path: file.corpus_path.clone(),
                        library_path: lib_path.to_string_lossy().to_string(),
                    }),
                    witness,
                );
            } else if conflict_paths.contains(file.deploy_path.as_str()) {
                // Deployed at wrong path, but target path is conflicted — skip
                conflict_skipped_count += 1;
            } else {
                // Deployed but at wrong path (stale) — mark as deploy-ready
                deploy_ready_count += 1;
                sender.write_typed_signal(
                    TypedSignalWrite::DeployReady(DeployReadySignal {
                        inode: file.inode,
                        path: file.corpus_path.clone(),
                        deploy_path: file.deploy_path.clone(),
                    }),
                    witness,
                );
            }
        } else if conflict_paths.contains(file.deploy_path.as_str()) {
            // Not deployed, and target path is conflicted — skip
            conflict_skipped_count += 1;
        } else {
            // Not deployed at all — deploy-ready
            deploy_ready_count += 1;
            sender.write_typed_signal(
                TypedSignalWrite::DeployReady(DeployReadySignal {
                    inode: file.inode,
                    path: file.corpus_path.clone(),
                    deploy_path: file.deploy_path.clone(),
                }),
                witness,
            );
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus: {} healthy files, {} deploy-ready, {} deployed-healthy, {} conflict-skipped, {} not configured",
        healthy_signals.len(),
        deploy_ready_count,
        deployed_healthy_count,
        conflict_skipped_count,
        skipped_not_configured,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
