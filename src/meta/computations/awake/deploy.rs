//! Deploy-related executors.
//!
//! Deploy conflict detection, deploy health signals, and corpus deploy status.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::helpers::{
    drop_stale_aggregate_signal,
    ensure_aggregate_signal_if_missing,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::{AggregateSignal, AggregateSignalType, CorpusFileSignalType, SignalType};
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
    sender.clear_signals_by_type(SignalType::DeployConflict, witness);

    let healthy_signals = read_only_db
        .get_signals(Some(SignalType::HealthyFile))
        .unwrap_or_default();

    let mut deploy_path_to_tracks: HashMap<String, Vec<i64>> = HashMap::new();

    for signal in &healthy_signals {
        let inode = match signal.inode {
            Some(i) => i,
            None => continue,
        };
        let corpus_path = signal.metadata_json.as_ref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|v| v.get("path").and_then(|p| p.as_str().map(String::from)))
            .unwrap_or_default();
        if corpus_path.is_empty() { continue; }

        let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
            .collect();

        let deploy_path = compute_deployment_path_with_tags(&corpus_path, &tag_map)
            .to_string_lossy()
            .to_string();

        deploy_path_to_tracks
            .entry(deploy_path)
            .or_default()
            .push(inode);
    }

    let mut conflict_count = 0;
    for (deploy_path, inodes) in deploy_path_to_tracks {
        if inodes.len() > 1 {
            conflict_count += 1;
            let signal = AggregateSignal {
                id: None,
                signal_type: AggregateSignalType::DeployConflict,
                key: deploy_path.clone(),
                discovered_at: None,
                metadata_json: Some(
                    serde_json::json!({
                        "deploy_path": deploy_path,
                    })
                    .to_string(),
                ),
            }
            .with_inodes(&inodes);

            sender.replace_aggregate_signal(signal, witness);
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

    // Convert to (path, inode) tuples for processing
    let library_files: Vec<(std::path::PathBuf, i64)> = library_scan_entries
        .into_iter()
        .map(|entry| (entry.file_path, entry.inode))
        .collect();

    // Get all corpus audio file inodes
    let corpus_inodes = read_only_db.get_all_corpus_inodes().unwrap_or_default();

    let mut healthy_count: usize = 0;
    let mut stale_count: usize = 0;
    let mut leftover_count: usize = 0;

    for (library_path, library_inode) in &library_files {
        let leftover_key = format!(
            "library_leftover:{}:{}",
            library_name,
            library_path.display()
        );
        let stale_key = format!(
            "library_stale:{}:{}",
            library_name,
            library_path.display()
        );

        if let Some(corpus_path) = corpus_inodes.get(library_inode) {
            drop_stale_aggregate_signal(read_only_db, &sender, AggregateSignalType::LibraryLeftover, &leftover_key, witness);

            // Check if stale and capture metadata for the signal
            let stale_metadata = if let Ok(Some(audio_file)) = read_only_db.get_audio_file_by_path(corpus_path) {
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
                    // Both paths stored as "{library_name}/path/..." for consistency
                    let expected_with_prefix = std::path::Path::new(library_name).join(&expected_relative);
                    Some(serde_json::json!({
                        "library_path": library_path.to_string_lossy(),
                        "expected_path": expected_with_prefix.to_string_lossy(),
                        "corpus_path": corpus_path,
                        "inode": inode
                    }))
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(metadata) = stale_metadata {
                stale_count += 1;
                ensure_aggregate_signal_if_missing(
                    read_only_db,
                    &sender,
                    AggregateSignalType::LibraryStale,
                    &stale_key,
                    Some(&metadata.to_string()),
                    witness,
                );
            } else {
                healthy_count += 1;
                drop_stale_aggregate_signal(read_only_db, &sender, AggregateSignalType::LibraryStale, &stale_key, witness);
            }
        } else {
            leftover_count += 1;
            ensure_aggregate_signal_if_missing(read_only_db, &sender, AggregateSignalType::LibraryLeftover, &leftover_key, None, witness);
            drop_stale_aggregate_signal(read_only_db, &sender, AggregateSignalType::LibraryStale, &stale_key, witness);
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
    sender.clear_corpus_signals_by_type(CorpusFileSignalType::DeployReady, witness);
    sender.clear_corpus_signals_by_type(CorpusFileSignalType::DeployedHealthy, witness);

    // Get all HealthyFile signals
    let healthy_signals = read_only_db
        .get_signals(Some(SignalType::HealthyFile))
        .unwrap_or_default();

    // Build inode → library paths map from files table (source='library')
    // This replaces the old stale_signals/stale_inodes approach that had a race
    // condition with DeriveDeployHealthSignals. We compute stale status inline.
    let mut library_inode_to_paths: HashMap<i64, Vec<PathBuf>> = HashMap::new();
    if let Ok(all_library_files) = read_only_db.get_all_library_files() {
        for entry in all_library_files {
            library_inode_to_paths
                .entry(entry.inode)
                .or_default()
                .push(entry.file_path);
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

    let mut deploy_ready_count = 0usize;
    let mut deployed_healthy_count = 0usize;
    let mut skipped_not_configured = 0usize;

    for signal in &healthy_signals {
        // Extract inode from signal's native inode column
        let inode = match signal.inode {
            Some(i) => i,
            None => continue,
        };
        // Extract corpus path from metadata_json
        let corpus_path = signal.metadata_json.as_ref()
            .and_then(|m| serde_json::from_str::<serde_json::Value>(m).ok())
            .and_then(|v| v.get("path").and_then(|p| p.as_str().map(String::from)))
            .unwrap_or_default();
        if corpus_path.is_empty() { continue; }

        let corpus_path_buf = Path::new(&corpus_path);

        // Skip files not in a configured source directory
        if !config.is_path_in_source(corpus_path_buf) {
            skipped_not_configured += 1;
            continue;
        }

        // Compute expected deploy path from tags
        let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
        let tag_map: HashMap<String, String> = tags
            .into_iter()
            .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
            .collect();
        let expected_relative = compute_deployment_path_with_tags(&corpus_path, &tag_map);

        // Check if this inode is deployed in any library
        if let Some(library_paths) = library_inode_to_paths.get(&inode) {
            // File is deployed — check if any library path matches expected
            let matching_path = library_paths.iter().find(|lp| {
                // Library paths are "{library_name}/{relative_path}"
                // Strip library name prefix for comparison with expected_relative
                let components: Vec<_> = lp.components().collect();
                if components.len() > 1 {
                    let suffix: PathBuf = components[1..].iter().collect();
                    suffix == expected_relative
                } else {
                    false
                }
            });

            if let Some(lib_path) = matching_path {
                // Correctly deployed
                deployed_healthy_count += 1;
                let metadata = serde_json::json!({
                    "library_path": lib_path.to_string_lossy(),
                });
                sender.ensure_corpus_signal_with_metadata(
                    CorpusFileSignalType::DeployedHealthy,
                    inode,
                    &corpus_path,
                    metadata,
                    witness,
                );
            } else {
                // Deployed but at wrong path (stale) — mark as deploy-ready
                deploy_ready_count += 1;
                let metadata = serde_json::json!({
                    "deploy_path": expected_relative.to_string_lossy(),
                });
                sender.ensure_corpus_signal_with_metadata(
                    CorpusFileSignalType::DeployReady,
                    inode,
                    &corpus_path,
                    metadata,
                    witness,
                );
            }
        } else {
            // Not deployed at all — deploy-ready
            deploy_ready_count += 1;
            let metadata = serde_json::json!({
                "deploy_path": expected_relative.to_string_lossy(),
            });
            sender.ensure_corpus_signal_with_metadata(
                CorpusFileSignalType::DeployReady,
                inode,
                &corpus_path,
                metadata,
                witness,
            );
        }
    }

    log_general(format!(
        "[COMPUTE] DeriveCorpusDeployStatus: {} healthy files, {} deploy-ready, {} deployed-healthy, {} not configured",
        healthy_signals.len(),
        deploy_ready_count,
        deployed_healthy_count,
        skipped_not_configured,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
