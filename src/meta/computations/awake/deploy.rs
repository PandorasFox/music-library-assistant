//! Deploy-related executors.
//!
//! Deploy conflict detection, deploy health signals, and corpus deploy status.

use std::collections::HashMap;
use std::path::Path;
use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::helpers::{
    drop_stale_aggregate_signal, drop_stale_corpus_signal,
    ensure_aggregate_signal_if_missing, ensure_corpus_signal,
    ensure_corpus_signal_with_metadata,
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
        let corpus_path = &signal.issue_key;
        if let Ok(Some(audio_file)) = read_only_db.get_audio_file_by_path(corpus_path) {
            let inode = audio_file.inode();
            let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
            let tag_map: HashMap<String, String> = tags
                .into_iter()
                .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                .collect();

            let deploy_path = compute_deployment_path_with_tags(corpus_path, &tag_map)
                .to_string_lossy()
                .to_string();

            deploy_path_to_tracks
                .entry(deploy_path)
                .or_default()
                .push(inode);
        }
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

    // Get all HealthyFile signals
    let healthy_signals = read_only_db
        .get_signals(Some(SignalType::HealthyFile))
        .unwrap_or_default();

    // Build set of deployed inodes from files table (source='library')
    let deployed_inodes = read_only_db.get_all_library_inodes().unwrap_or_default();

    // Get set of stale library paths (files in library but at wrong path)
    // LibraryStale keys have format "library_stale:{library_name}:{library_path}"
    let stale_signals = read_only_db
        .get_signals(Some(SignalType::LibraryStale))
        .unwrap_or_default();

    // Build set of inodes that are deployed but stale
    let mut stale_inodes: std::collections::HashSet<i64> = std::collections::HashSet::new();
    if let Ok(all_library_files) = read_only_db.get_all_library_files() {
        for entry in all_library_files {
            // Check if this library file has a stale signal
            let is_stale = stale_signals.iter().any(|s| {
                // Parse the stale key to get library_path
                let parts: Vec<&str> = s.issue_key.splitn(3, ':').collect();
                if parts.len() >= 3 {
                    let stale_library_path = parts[2];
                    entry.file_path.to_string_lossy() == stale_library_path
                } else {
                    false
                }
            });
            if is_stale {
                stale_inodes.insert(entry.inode);
            }
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
        let corpus_path = &signal.issue_key;
        let corpus_path_buf = std::path::Path::new(corpus_path);

        // Get the audio file (we need inode for signals and tags lookup)
        let audio_file = match read_only_db.get_audio_file_by_path(corpus_path) {
            Ok(Some(af)) => af,
            _ => continue, // Skip if file not found
        };
        let inode = audio_file.inode();

        // Skip files not in a configured source directory
        if !config.is_path_in_source(corpus_path_buf) {
            skipped_not_configured += 1;
            // Clear any stale deploy signals for unconfigured files
            drop_stale_corpus_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::DeployReady,
                inode,
                witness,
            );
            drop_stale_corpus_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::DeployedHealthy,
                inode,
                witness,
            );
            continue;
        }

        // Check if this inode is deployed anywhere and not stale
        let is_deployed = deployed_inodes.contains(&inode);
        let is_stale = stale_inodes.contains(&inode);

        if is_deployed && !is_stale {
            // File is correctly deployed
            deployed_healthy_count += 1;
            ensure_corpus_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::DeployedHealthy,
                inode,
                corpus_path,
                witness,
            );
            drop_stale_corpus_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::DeployReady,
                inode,
                witness,
            );
        } else {
            // File is not deployed (or deployed but stale)
            deploy_ready_count += 1;

            // Compute the deploy path for this file (relative)
            let tags = read_only_db.get_corpus_tags(inode).unwrap_or_default();
            let tag_map: HashMap<String, String> = tags
                .into_iter()
                .map(|t| (t.tag_name.to_lowercase(), t.tag_value))
                .collect();
            // Compute relative deploy path (relative to library root)
            let deploy_path = compute_deployment_path_with_tags(corpus_path, &tag_map);

            // Store relative path in metadata
            let metadata = serde_json::json!({
                "deploy_path": deploy_path.to_string_lossy(),
            });
            ensure_corpus_signal_with_metadata(
                read_only_db,
                &sender,
                CorpusFileSignalType::DeployReady,
                inode,
                corpus_path,
                metadata,
                witness,
            );
            drop_stale_corpus_signal(
                read_only_db,
                &sender,
                CorpusFileSignalType::DeployedHealthy,
                inode,
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
