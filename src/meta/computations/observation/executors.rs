//! Observation-phase computation executors.
//!
//! These functions implement the actual logic for Observation computations.

use std::path::Path;

use crate::db::types::Zone;
use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::helpers::{
    drop_stale_corpus_signal, ensure_typed_signal, extract_mtime,
};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::*;
use crate::meta::signals::registry::TypedSignalWrite;

use super::{Computation, Result};

// ============================================================================
// Verify Mtime
// ============================================================================

/// Phase 3: Verify single file mtime.
pub fn execute_verify_mtime(
    _read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
    expected_mtime_secs: i64,
    expected_mtime_nanos: i64,
) -> Result {
    let computation = Computation::VerifyMtime {
        inode,
        path: path.to_path_buf(),
        expected_mtime_secs,
        expected_mtime_nanos,
    };

    // Read current mtime from disk
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to read file metadata: {}", e),
            );
        }
    };

    let (current_secs, current_nanos) = extract_mtime(&metadata);

    // If mtime differs from expected, spawn VerifyTags to check actual content
    let spawn = if current_secs != expected_mtime_secs || current_nanos != expected_mtime_nanos {
        vec![Computation::VerifyTags {
            inode,
            path: path.to_path_buf(),
        }]
    } else {
        Vec::new()
    };

    Result::success(computation, spawn)
}

/// Check if a file's disk mtime differs from what's stored in files table.
///
/// Uses the portable API (extract_mtime) for consistency across the codebase.
/// Returns true if mtime differs or if we can't determine (fail-safe to emit signal).
/// Resolves the file's zone from the DB rather than hardcoding, so this works
/// correctly for both corpus and inbox files.
fn check_mtime_differs(read_only_db: &ReadOnlyDb<'_>, inode: i64, path: &Path) -> bool {
    // Resolve the file's zone from the DB
    let zone = read_only_db
        .get_file_zone_and_path_by_inode(inode)
        .ok()
        .flatten()
        .and_then(|(z, _)| Zone::from_str(&z))
        .unwrap_or(Zone::Corpus);

    // Get mtime info from files table for this inode
    let mtime_info = match read_only_db.get_file_mtime_batch(zone, &[inode]) {
        Ok(map) => match map.get(&inode) {
            Some((db_secs, db_nanos)) => (*db_secs, *db_nanos),
            None => return true, // Not in files table, assume differs
        },
        Err(_) => return true, // Query failed, assume differs
    };

    // Get current disk mtime using portable API
    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return true, // Can't read file, assume differs
    };
    let (disk_secs, disk_nanos) = extract_mtime(&metadata);

    // Compare
    mtime_info.0 != disk_secs || mtime_info.1 != disk_nanos
}

// ============================================================================
// Verify Tags
// ============================================================================

/// Verify tags on disk match database and classify OOB changes.
///
/// Performs full classification of out-of-band changes directly:
/// - `MtimeOnlyMismatch`: mtime changed but tags are identical (requires operator acknowledgement)
/// - `OutOfBandTagSync`: one-direction tag extras only (syncable without conflict)
/// - `OutOfBandTagConflict`: value conflicts or mixed-direction extras (requires operator decision)
///
/// These three signal types are mutually exclusive - emitting one clears the others.
pub fn execute_verify_tags(
    read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
    witness: &ComputationWitness,
) -> Result {
    use crate::meta::mutations::indexing;

    let computation = Computation::VerifyTags {
        inode,
        path: path.to_path_buf(),
    };

    // Route mismatch writes through db_thread (read-only connection can't write directly)
    let sender = match crate::db::write_thread::signal_sender().cloned() {
        Some(s) => s,
        None => {
            return Result::failure(
                computation,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Wait for pending DB writes to drain before reading audio_info.
    // This prevents a race where VerifyTags runs before IndexFileFromPath's
    // fire-and-forget write is processed, causing spurious "Audio file not found" errors.
    crate::db::write_thread::wait_for_queue_drain();

    // Get relative path for signal keys
    let resolver = crate::corpus::paths::get_resolver();
    let rel_path = match resolver.to_relative(path) {
        Some(rel) => rel,
        None => {
            return Result::failure(
                computation,
                format!("Path {} does not match any configured root", path.display()),
            );
        }
    };
    let rel_str = rel_path.to_string_lossy().to_string();

    match indexing::execute_verify_tags(read_only_db, inode, path) {
        Ok(verify_result) => {
            // Full classification based on TagVerifyResult
            // All signals are now keyed by inode with path in metadata
            if verify_result.is_clean() {
                // Tags match exactly - but we need to check if mtime actually differs
                // (in force_check mode, VerifyTags runs even when mtime matches)
                let mtime_actually_differs = check_mtime_differs(read_only_db, inode, path);

                if mtime_actually_differs {
                    // Mtime changed but tags are identical - requires operator acknowledgement
                    log_general(format!(
                        "[COMPUTE] VerifyTags: mtime-only change for inode {} ({})",
                        inode,
                        path.display()
                    ));
                    ensure_typed_signal(
                        read_only_db,
                        &sender,
                        TypedSignalWrite::MtimeOnlyMismatch(MtimeOnlyMismatchSignal {
                            inode,
                            path: rel_str.clone(),
                        }),
                        witness,
                    );
                    // Clear mutually exclusive signals
                    drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                } else {
                    // Tags match AND mtime matches - file is healthy, clear all OOB signals
                    log_general(format!(
                        "[COMPUTE] VerifyTags: file healthy for inode {} ({})",
                        inode,
                        path.display()
                    ));
                    drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                        read_only_db,
                        &sender,
                        inode,
                        witness,
                    );
                }
            } else if verify_result.is_conflict() {
                // Value conflicts or mixed-direction extras - requires operator decision
                log_general(format!(
                    "[COMPUTE] VerifyTags: tag conflict for inode {} ({})",
                    inode,
                    path.display()
                ));
                // Clear all OOB signals first (including target type to refresh metadata)
                drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                // Create fresh signal with mismatch metadata (keyed by inode)
                let mismatches: Vec<TagMismatchEntry> = verify_result
                    .mismatches
                    .into_iter()
                    .map(|m| TagMismatchEntry {
                        tag_name: m.field,
                        disk_value: m.disk_value,
                        db_value: m.db_value,
                    })
                    .collect();
                sender.write_typed_signal(
                    TypedSignalWrite::OutOfBandTagConflict(OutOfBandTagConflictSignal {
                        inode,
                        path: rel_str.clone(),
                        mismatches,
                    }),
                    witness,
                );
            } else {
                // One-direction extras only - can be synced without conflict
                log_general(format!(
                    "[COMPUTE] VerifyTags: syncable tag diff for inode {} ({})",
                    inode,
                    path.display()
                ));
                // Clear all OOB signals first (including target type to refresh metadata)
                drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                    read_only_db,
                    &sender,
                    inode,
                    witness,
                );
                // Create fresh signal with mismatch metadata (keyed by inode)
                let mismatches: Vec<TagMismatchEntry> = verify_result
                    .mismatches
                    .into_iter()
                    .map(|m| TagMismatchEntry {
                        tag_name: m.field,
                        disk_value: m.disk_value,
                        db_value: m.db_value,
                    })
                    .collect();
                sender.write_typed_signal(
                    TypedSignalWrite::OutOfBandTagSync(OutOfBandTagSyncSignal {
                        inode,
                        path: rel_str.clone(),
                        mismatches,
                    }),
                    witness,
                );
            }

            Result::success(computation, Vec::new())
        }
        Err(e) => {
            // Emit CorruptFile signal so the issue is tracked in the DB (actionable)
            // Log to both general and errors so we can diagnose why this file is flagged
            let err_msg = format!(
                "[COMPUTE] VerifyTags FAILED for inode {} ({}): {:#}",
                inode,
                path.display(),
                e
            );
            log_general(&err_msg);
            crate::logging::log_error(&err_msg);
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::CorruptFile(CorruptFileSignal {
                    inode,
                    path: rel_str.clone(),
                }),
                witness,
            );
            // Clear OOB signals on parse error - we can't classify what we can't read
            drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                read_only_db,
                &sender,
                inode,
                witness,
            );
            // Return success so computation continues processing other files
            Result::success(computation, Vec::new())
        }
    }
}

// ============================================================================
// Verify Audio (Deep Integrity Check)
// ============================================================================

/// Verify audio file integrity by decoding the entire stream.
///
/// Catches truncated files, corrupt audio data, and other issues that
/// tag verification wouldn't detect. Emits CorruptFile if decode fails.
pub fn execute_verify_audio(
    read_only_db: &ReadOnlyDb<'_>,
    inode: i64,
    path: &Path,
    witness: &ComputationWitness,
) -> Result {
    let computation = Computation::VerifyAudio {
        inode,
        path: path.to_path_buf(),
    };

    // Get signal sender for async writes
    let sender = match crate::db::write_thread::signal_sender().cloned() {
        Some(s) => s,
        None => {
            return Result::failure(
                computation,
                "DB thread not initialized".to_string(),
            );
        }
    };

    // Get relative path for signal keys
    let resolver = crate::corpus::paths::get_resolver();
    let rel_path = match resolver.to_relative(path) {
        Some(rel) => rel,
        None => {
            return Result::failure(
                computation,
                format!("Path {} does not match any configured root", path.display()),
            );
        }
    };
    let rel_str = rel_path.to_string_lossy().to_string();

    // Verify audio integrity by decoding the entire file
    match crate::corpus::metadata::verify_audio_integrity(path) {
        Ok(()) => {
            // Audio is valid - clear any stale CorruptFile signal (keyed by inode)
            drop_stale_corpus_signal::<CorruptFileSignal>(read_only_db, &sender, inode, witness);
            Result::success(computation, Vec::new())
        }
        Err(e) => {
            // Audio verification failed - file is corrupt
            // Log to both general and errors so we can diagnose why this file is flagged
            let err_msg = format!(
                "[COMPUTE] VerifyAudio FAILED for inode {} ({}): {:#}",
                inode,
                path.display(),
                e
            );
            log_general(&err_msg);
            crate::logging::log_error(&err_msg);
            ensure_typed_signal(
                read_only_db,
                &sender,
                TypedSignalWrite::CorruptFile(CorruptFileSignal {
                    inode,
                    path: rel_str.clone(),
                }),
                witness,
            );
            // Return success so computation continues processing other files
            // (the signal emission handles the error state)
            Result::success(computation, Vec::new())
        }
    }
}
