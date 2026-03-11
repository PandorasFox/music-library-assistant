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
///
/// **Pending write awareness**: When a `pending_write` marker exists for this inode
/// (set by `write_file_tags()` before MM writes), the mtime check is skipped on
/// clean tags (expected: MM just wrote them). This prevents spurious MtimeOnlyMismatch
/// signals for MM-initiated writes.
///
/// **Mtime update**: All branches update `files.mtime` from current disk state,
/// since `write_file_tags()` no longer handles mtime updates.
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
    // This also guarantees any pending_write marker from write_file_tags() is committed.
    crate::db::write_thread::wait_for_queue_drain();

    // Check for pending_write marker (MM-initiated write)
    let has_pending_write = read_only_db.is_pending_write(inode).unwrap_or(false);

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

    // Resolve zone for mtime update (needed in all branches)
    let zone_str = read_only_db
        .get_file_zone_and_path_by_inode(inode)
        .ok()
        .flatten()
        .map(|(z, _)| z)
        .unwrap_or_else(|| "corpus".to_string());

    match indexing::execute_verify_tags(read_only_db, inode, path) {
        Ok(verify_result) => {
            if verify_result.is_clean() {
                if has_pending_write {
                    // MM wrote tags and they match — write succeeded. No signal needed.
                    log_general(format!(
                        "[COMPUTE] VerifyTags: pending_write succeeded for inode {} ({})",
                        inode,
                        path.display()
                    ));
                    // Clear all OOB signals (MM write reconciled everything)
                    drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                        read_only_db, &sender, inode, witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                        read_only_db, &sender, inode, witness,
                    );
                    drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                        read_only_db, &sender, inode, witness,
                    );
                } else {
                    // No pending_write — external or idle rescan path
                    let mtime_actually_differs = check_mtime_differs(read_only_db, inode, path);

                    if mtime_actually_differs {
                        // Mtime changed but tags identical — external touch
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
                        drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                            read_only_db, &sender, inode, witness,
                        );
                        drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                            read_only_db, &sender, inode, witness,
                        );
                    } else {
                        // Tags match AND mtime matches — file is healthy
                        log_general(format!(
                            "[COMPUTE] VerifyTags: file healthy for inode {} ({})",
                            inode,
                            path.display()
                        ));
                        drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                            read_only_db, &sender, inode, witness,
                        );
                        drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                            read_only_db, &sender, inode, witness,
                        );
                        drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                            read_only_db, &sender, inode, witness,
                        );
                    }
                }
            } else if verify_result.is_conflict() {
                if has_pending_write {
                    log_general(format!(
                        "[COMPUTE] VerifyTags: pending_write but tags conflict for inode {} ({}) — post-write external modification or write failure",
                        inode, path.display()
                    ));
                }
                // Value conflicts or mixed-direction extras — requires operator decision
                log_general(format!(
                    "[COMPUTE] VerifyTags: tag conflict for inode {} ({})",
                    inode,
                    path.display()
                ));
                // Clear all OOB signals first (including target type to refresh metadata)
                drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                    read_only_db, &sender, inode, witness,
                );
                drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                    read_only_db, &sender, inode, witness,
                );
                drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                    read_only_db, &sender, inode, witness,
                );
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
                if has_pending_write {
                    log_general(format!(
                        "[COMPUTE] VerifyTags: pending_write but syncable diff for inode {} ({}) — post-write external modification or write failure",
                        inode, path.display()
                    ));
                }
                // One-direction extras only — can be synced without conflict
                log_general(format!(
                    "[COMPUTE] VerifyTags: syncable tag diff for inode {} ({})",
                    inode,
                    path.display()
                ));
                drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                    read_only_db, &sender, inode, witness,
                );
                drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                    read_only_db, &sender, inode, witness,
                );
                drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                    read_only_db, &sender, inode, witness,
                );
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

            // Consume pending_write marker regardless of outcome
            if has_pending_write {
                sender.clear_dirty_inode(inode, "pending_write", witness);
            }

            // Update DB mtime from current disk state (all branches)
            if let Ok(metadata) = std::fs::metadata(path) {
                let (secs, nanos) = extract_mtime(&metadata);
                sender.update_file_mtime(&zone_str, inode, secs, nanos, witness);
            }

            Result::success(computation, Vec::new())
        }
        Err(e) => {
            // Emit CorruptFile signal so the issue is tracked in the DB (actionable)
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
            // Clear OOB signals on parse error — can't classify what we can't read
            drop_stale_corpus_signal::<OutOfBandTagConflictSignal>(
                read_only_db, &sender, inode, witness,
            );
            drop_stale_corpus_signal::<OutOfBandTagSyncSignal>(
                read_only_db, &sender, inode, witness,
            );
            drop_stale_corpus_signal::<MtimeOnlyMismatchSignal>(
                read_only_db, &sender, inode, witness,
            );

            // Consume pending_write marker regardless
            if has_pending_write {
                sender.clear_dirty_inode(inode, "pending_write", witness);
            }

            // Update DB mtime even on error (prevents re-triggering)
            if let Ok(metadata) = std::fs::metadata(path) {
                let (secs, nanos) = extract_mtime(&metadata);
                sender.update_file_mtime(&zone_str, inode, secs, nanos, witness);
            }

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
