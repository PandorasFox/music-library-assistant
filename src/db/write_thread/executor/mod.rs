//! DB thread executor — processes `DbWriteOp` messages on the write connection.
//!
//! Contains the main execution loop, retry logic, signal clearing helpers,
//! and all `execute_*` functions that perform the actual SQL operations.

mod external_ops;
mod index_ops;
mod packing_ops;

use rusqlite::params;

use crate::db::Database;
use crate::meta::signals::data::*;
use crate::meta::signals::store::CorpusSignalStore;

use super::DbWriteOp;

// ============================================================================
// Retry Logic
// ============================================================================

/// Retry constants for transient SQLite errors.
const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 50;

/// Check if an error is a retryable SQLite error and return the reason if so.
fn retryable_sqlite_error(e: &anyhow::Error) -> Option<String> {
    e.chain().find_map(|cause| {
        if let Some(sqlite_err) = cause.downcast_ref::<rusqlite::Error>() {
            match sqlite_err {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ffi::ErrorCode::DatabaseBusy,
                        extended_code,
                    },
                    msg,
                ) => Some(format!("SQLITE_BUSY (ext={}): {:?}", extended_code, msg)),
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error {
                        code: rusqlite::ffi::ErrorCode::DatabaseLocked,
                        extended_code,
                    },
                    msg,
                ) => Some(format!("SQLITE_LOCKED (ext={}): {:?}", extended_code, msg)),
                _ => None,
            }
        } else {
            None
        }
    })
}

/// Execute an operation with retry logic for transient SQLite errors.
fn with_retry<F>(op_name: &str, context: &str, mut f: F)
where
    F: FnMut() -> anyhow::Result<()>,
{
    for attempt in 0..=MAX_RETRIES {
        match f() {
            Ok(()) => return,
            Err(e) => {
                if let Some(reason) = retryable_sqlite_error(&e) {
                    if attempt < MAX_RETRIES {
                        let delay = BASE_DELAY_MS * (1 << attempt);
                        crate::logging::log_error(format!(
                            "[DB_THREAD] {} retry {}/{} after {}ms: {} | {}",
                            op_name,
                            attempt + 1,
                            MAX_RETRIES,
                            delay,
                            reason,
                            context
                        ));
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    } else {
                        crate::logging::log_error(format!(
                            "[DB_THREAD] {} FAILED after {} retries: {} | {}",
                            op_name, MAX_RETRIES, reason, context
                        ));
                        return;
                    }
                } else {
                    // Non-retryable error
                    crate::logging::log_error(format!(
                        "[DB_THREAD] {} failed (non-retryable): {} | {}",
                        op_name, e, context
                    ));
                    return;
                }
            }
        }
    }
}

// ============================================================================
// Typed Signal Table Helpers
// ============================================================================

/// Clear corpus signals for an inode from typed tables.
///
/// When `include_inherent` is true, also clears file-inherent signals
/// (CorruptFile, ShitFormat). These represent intrinsic file properties
/// discovered during indexing and should persist across mutations that don't
/// remove/replace the file; pass `false` to preserve them.
fn typed_clear_corpus_signals(db: &Database, inode: i64, include_inherent: bool) {
    let conn = db.conn();
    let _ = FileInCorpusSignal::clear_by_inode(conn, inode);
    let _ = UnindexedFileSignal::clear_by_inode(conn, inode);
    let _ = HealthyFileSignal::clear_by_inode(conn, inode);
    let _ = MissingFileSignal::clear_by_inode(conn, inode);
    let _ = MissingDirectorySignal::clear_by_inode(conn, inode);
    let _ = MovedFileSignal::clear_by_inode(conn, inode);
    let _ = OutOfBandTagSyncSignal::clear_by_inode(conn, inode);
    let _ = OutOfBandTagConflictSignal::clear_by_inode(conn, inode);
    let _ = MtimeOnlyMismatchSignal::clear_by_inode(conn, inode);
    if include_inherent {
        let _ = CorruptFileSignal::clear_by_inode(conn, inode);
        let _ = ShitFormatSignal::clear_by_inode(conn, inode);
    }
    let _ = SubparDuplicateSignal::clear_by_inode(conn, inode);
    let _ = CompoundTagSignal::clear_by_inode(conn, inode);
    let _ = DeployReadySignal::clear_by_inode(conn, inode);
    let _ = DeployedHealthySignal::clear_by_inode(conn, inode);
}

// ============================================================================
// Inbox / Dirty / Library Operations
// ============================================================================

/// Execute DropInboxFileState: cascade-drop all inbox state for an inode.
///
/// Called when DeriveInboxSignals detects an inode that was indexed as inbox
/// but is no longer observed on disk. Cleans up:
/// - inbox_tags rows
/// - files table entry (zone='inbox' only)
/// - Per-inode inbox signals: FileInInbox, InboxUnindexed, InboxHealthy, InboxCorpusMatch
/// - MovedFile signal (inbox->X moves become disappear+reappear instead)
///
/// Does NOT touch audio_info (corpus may reference same inode after mv)
/// or corpus signals (FileInCorpus, HealthyFile, etc.).
fn execute_drop_inbox_file_state(db: &Database, inode: i64) -> anyhow::Result<()> {

    let conn = db.conn();

    // Delete inbox tags for this inode
    conn.execute("DELETE FROM inbox_tags WHERE inode = ?1", params![inode])?;

    // Delete inbox file entry (only zone='inbox', not corpus)
    conn.execute(
        "DELETE FROM files WHERE zone = 'inbox' AND inode = ?1",
        params![inode],
    )?;

    // Clear per-inode inbox signals
    let _ = FileInInboxSignal::clear_by_inode(conn, inode);
    let _ = InboxUnindexedSignal::clear_by_inode(conn, inode);
    let _ = InboxHealthySignal::clear_by_inode(conn, inode);
    let _ = InboxCorpusMatchSignal::clear_by_inode(conn, inode);

    // Clear MovedFile — inbox->X moves become disappear+reappear
    let _ = MovedFileSignal::clear_by_inode(conn, inode);

    Ok(())
}

/// Execute ClearDirtyInode: remove dirty flag after successful computation.
fn execute_clear_dirty_inode(
    db: &Database,
    inode: i64,
    computation_type: &str,
) -> anyhow::Result<()> {

    db.conn().execute(
        "DELETE FROM dirty_inodes WHERE inode = ?1 AND computation_type = ?2",
        params![inode, computation_type],
    )?;

    Ok(())
}

/// Execute MarkDirtyInodes: mark a batch of inodes dirty for a computation type.
fn execute_mark_dirty_inodes(
    db: &Database,
    inodes: &[i64],
    computation_type: &str,
) -> anyhow::Result<()> {
    let now = index_ops::current_unix_secs();
    for inode in inodes {
        db.conn().execute(
            "INSERT OR IGNORE INTO dirty_inodes (inode, computation_type, dirtied_at) VALUES (?1, ?2, ?3)",
            params![inode, computation_type, now],
        )?;
    }

    Ok(())
}

/// Execute UpsertLibraryFile: insert or update a library file in the files table.
fn execute_upsert_library_file(
    db: &Database,
    stored_path: &str,
    inode: i64,
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: i64,
) -> anyhow::Result<()> {
    let scanned_at = index_ops::current_unix_secs();

    db.conn().execute(
        "INSERT OR REPLACE INTO files
         (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
         VALUES (?1, 'library', ?2, 0, ?3, ?4, ?5, ?6)",
        params![
            inode,
            stored_path,
            mtime_secs,
            mtime_nanos,
            file_size,
            scanned_at
        ],
    )?;

    Ok(())
}

/// Execute DeleteLibraryFile: remove a stale library file from the files table.
fn execute_delete_library_file(db: &Database, stored_path: &str) -> anyhow::Result<()> {

    db.conn().execute(
        "DELETE FROM files WHERE zone = 'library' AND path = ?1",
        params![stored_path],
    )?;

    Ok(())
}

// ============================================================================
// Main Executor Dispatch
// ============================================================================

/// Execute a single signal write operation.
pub(super) fn execute_signal_op(db: &Database, op: &DbWriteOp) {
    // Note: We don't have a ComputationWitness here, but we need one for the db methods.
    // The witness was checked at the send site. We use a thread-local witness for execution.
    let witness = crate::meta::computations::ComputationWitness::new_for_db_thread();

    match op {
        // Signal clear operations (function-pointer dispatch)
        DbWriteOp::ClearCorpusSignalByInode {
            clear_fn,
            inode,
            label,
        } => {
            if let Err(e) = clear_fn(db.conn(), *inode) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] clear {} by inode {} failed: {}",
                    label, inode, e
                ));
            }
        }
        DbWriteOp::ClearAllCorpusSignals { inode } => {
            typed_clear_corpus_signals(db, *inode, true);
        }
        DbWriteOp::ClearMutableCorpusSignals { inode } => {
            typed_clear_corpus_signals(db, *inode, false);
        }
        DbWriteOp::ClearAggregateSignalByKey {
            clear_fn,
            key,
            label,
        } => {
            if let Err(e) = clear_fn(db.conn(), key) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] clear {} by key '{}' failed: {}",
                    label, key, e
                ));
            }
        }
        DbWriteOp::ClearAggregateByKeyPrefix {
            clear_fn,
            prefix,
            label,
        } => {
            if let Err(e) = clear_fn(db.conn(), prefix) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] clear {} by prefix '{}' failed: {}",
                    label, prefix, e
                ));
            }
        }

        DbWriteOp::ClearSignalTable { clear_fn, label } => {
            match clear_fn(db.conn()) {
                Ok(count) => {
                    if count > 0 {
                        crate::logging::log_general(format!(
                            "[DB_THREAD] cleared {} rows from {}",
                            count, label
                        ));
                    }
                }
                Err(e) => {
                    crate::logging::log_error(format!(
                        "[DB_THREAD] clear all {} failed: {}",
                        label, e
                    ));
                }
            }
        }

        // Library file operations (Awakening phase - reconciliation)
        DbWriteOp::UpsertLibraryFile {
            stored_path,
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
        } => {
            with_retry("upsert_library_file", stored_path, || {
                execute_upsert_library_file(
                    db,
                    stored_path,
                    *inode,
                    *mtime_secs,
                    *mtime_nanos,
                    *file_size,
                )
            });
        }
        DbWriteOp::DeleteLibraryFile { stored_path } => {
            with_retry("delete_library_file", stored_path, || {
                execute_delete_library_file(db, stored_path)
            });
        }

        DbWriteOp::WriteTypedSignal { signal } => {
            if let Err(e) = signal.clone().insert(db.conn()) {
                crate::logging::log_error(format!("[DB_THREAD] write_typed_signal failed: {}", e));
            }
        }
        DbWriteOp::WriteTypedSignalBatch { signals } => {
            for signal in signals {
                if let Err(e) = signal.clone().insert(db.conn()) {
                    crate::logging::log_error(format!(
                        "[DB_THREAD] write_typed_signal_batch item failed: {}",
                        e
                    ));
                }
            }
        }
        DbWriteOp::UpdateFileMtime {
            zone,
            inode,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("update_file_mtime", zone, || {
                let rows_affected = db.conn()
                    .execute(
                        "UPDATE files SET mtime_secs = ?1, mtime_nanos = ?2 WHERE zone = ?3 AND inode = ?4",
                        params![mtime_secs, mtime_nanos, zone, inode],
                    )
                    .map_err(|e: rusqlite::Error| anyhow::anyhow!(e))?;
                if rows_affected == 0 {
                    crate::logging::log_error(format!(
                        "[DB_THREAD] update_file_mtime: no rows matched for zone={}, inode={}",
                        zone, inode
                    ));
                }
                Ok(())
            });
        }

        // =====================================================================
        // File/Audio Index Operations (Mutation execution)
        // =====================================================================
        DbWriteOp::IndexAudioFile {
            path,
            file_data,
            audio_data,
            tags,
            session_id,
        } => {
            with_retry("index_audio_file", path, || {
                index_ops::execute_index_audio_file(db, path, file_data, audio_data, tags, session_id)
            });
        }

        DbWriteOp::DropFromIndex { path } => {
            with_retry("drop_from_index", path, || {
                index_ops::execute_drop_from_index(db, path)
            });
        }

        DbWriteOp::SetIndexTrackTags {
            path,
            tags,
            tag_table,
            session_id,
        } => {
            with_retry("set_index_track_tags", path, || {
                index_ops::execute_set_index_track_tags(db, path, tags, tag_table, session_id)
            });
        }

        DbWriteOp::ApplyIndexTagOps {
            path,
            ops,
            tag_table,
            session_id,
        } => {
            with_retry("apply_index_tag_ops", path, || {
                index_ops::execute_apply_index_tag_ops(db, path, ops, tag_table, session_id)
            });
        }

        DbWriteOp::UpdateTrackPathWithMetadata {
            old_path,
            new_path,
            new_inode,
            new_file_size,
            new_file_type,
        } => {
            with_retry("update_track_path_with_metadata", old_path, || {
                index_ops::execute_update_track_path_with_metadata(
                    db,
                    old_path,
                    new_path,
                    *new_inode,
                    *new_file_size,
                    new_file_type,
                )
            });
        }

        DbWriteOp::UpsertFileEntry {
            path,
            zone,
            file_entry,
        } => {
            with_retry("upsert_file_entry", path, || {
                index_ops::execute_upsert_file_entry(db, path, zone, file_entry)
            });
        }

        DbWriteOp::DropFileIndexByInode { zone, inode } => {
            with_retry("drop_file_index_by_inode", zone, || {
                db.drop_file_index_by_inode(zone, *inode, &witness)
                    .map(|_| ())
            });
        }

        DbWriteOp::UpdateFilePath {
            zone,
            inode,
            new_path,
            new_zone,
        } => {
            with_retry("update_file_path", new_path, || {
                db.update_file_path(zone, *inode, new_path, &witness)
            });
            // Cross-zone move: update zone column and migrate tags
            if let Some(nz) = new_zone {
                if nz != zone {
                    with_retry("update_file_zone", nz, || {
                        db.update_file_zone(zone, *inode, nz, &witness)
                    });
                }
            }
        }

        DbWriteOp::IndexDirectory {
            path,
            zone,
            inode,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("index_directory", path, || {
                index_ops::execute_index_directory(db, path, zone, *inode, *mtime_secs, *mtime_nanos)
            });
        }

        DbWriteOp::IndexImageFile {
            path,
            zone,
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
        } => {
            with_retry("index_image_file", path, || {
                index_ops::execute_index_image_file(
                    db,
                    path,
                    zone,
                    *inode,
                    *mtime_secs,
                    *mtime_nanos,
                    *file_size,
                )
            });
        }

        DbWriteOp::UpsertImageInfo {
            inode,
            format,
            width,
            height,
            role,
        } => {
            with_retry("upsert_image_info", format, || {
                index_ops::execute_upsert_image_info(db, *inode, format, *width, *height, role)
            });
        }

        DbWriteOp::ClearTagMismatchesForTrack { path } => {
            with_retry("clear_tag_mismatches_for_track", path, || {
                index_ops::execute_clear_tag_mismatches_for_track(db, path)
            });
        }

        DbWriteOp::SetNeedsDiskFlush { path, value } => {
            with_retry("set_needs_disk_flush", path, || {
                index_ops::execute_set_needs_disk_flush(db, path, *value)
            });
        }

        DbWriteOp::DropInboxFileState { inode } => {
            with_retry("drop_inbox_file_state", &inode.to_string(), || {
                execute_drop_inbox_file_state(db, *inode)
            });
        }

        DbWriteOp::ClearDirtyInode {
            inode,
            computation_type,
        } => {
            with_retry("clear_dirty_inode", computation_type, || {
                execute_clear_dirty_inode(db, *inode, computation_type)
            });
        }

        DbWriteOp::MarkDirtyInodes {
            inodes,
            computation_type,
        } => {
            with_retry("mark_dirty_inodes", computation_type, || {
                execute_mark_dirty_inodes(db, inodes, computation_type)
            });
        }

        DbWriteOp::InsertExternalMatch {
            inode,
            fingerprint,
            source,
            recording_id,
            confidence,
            raw_response,
            fetched_at,
        } => {
            with_retry("insert_external_match", recording_id, || {
                external_ops::execute_insert_external_match(
                    db,
                    *inode,
                    fingerprint,
                    *source,
                    recording_id,
                    *confidence,
                    raw_response.as_deref(),
                    *fetched_at,
                )
            });
        }

        DbWriteOp::InsertExternalNoMatch {
            fingerprint,
            source,
            queried_at,
        } => {
            with_retry("insert_external_no_match", &source.to_string(), || {
                external_ops::execute_insert_external_no_match(db, fingerprint, *source, *queried_at)
            });
        }

        DbWriteOp::UpsertExternalRetry {
            inode,
            fingerprint,
            source,
            error,
        } => {
            with_retry("upsert_external_retry", error, || {
                external_ops::execute_upsert_external_retry(db, *inode, fingerprint, *source, error)
            });
        }

        DbWriteOp::DeleteExternalRetry { inode, source } => {
            with_retry("delete_external_retry", &inode.to_string(), || {
                external_ops::execute_delete_external_retry(db, *inode, *source)
            });
        }

        DbWriteOp::DropExternalMatch { inode } => {
            with_retry("drop_external_match", &inode.to_string(), || {
                external_ops::execute_drop_external_match(db, *inode)
            });
        }

        DbWriteOp::UpsertMbCache {
            table,
            id_col,
            id,
            raw_json,
            fetched_at,
        } => {
            with_retry("upsert_mb_cache", id, || {
                external_ops::execute_upsert_mb_cache(db, table, id_col, id, raw_json, *fetched_at)
            });
        }

        DbWriteOp::InsertMbKnownEntity {
            mbid,
            entity_type,
            discovered_from,
            discovered_at,
        } => {
            with_retry("insert_mb_known_entity", mbid, || {
                external_ops::execute_insert_mb_known_entity(
                    db,
                    mbid,
                    entity_type,
                    discovered_from.as_deref(),
                    *discovered_at,
                )
            });
        }

        DbWriteOp::ClearTagEditHistory => {
            with_retry("clear_tag_edit_history", "all", || {
                db.conn()
                    .execute("DELETE FROM tag_edit_history", [])
                    .map(|_| ())
                    .map_err(Into::into)
            });
        }

        DbWriteOp::ClearTagEditHistorySession { session_id } => {
            with_retry("clear_tag_edit_history_session", session_id, || {
                db.conn()
                    .execute(
                        "DELETE FROM tag_edit_history WHERE session_id = ?1",
                        rusqlite::params![session_id],
                    )
                    .map(|_| ())
                    .map_err(Into::into)
            });
        }

        // ExecuteVacuum, ApplyReconciliation, and Shutdown are handled in the run_db_thread loop, never reach here
        DbWriteOp::ExecuteVacuum { .. } => {
            unreachable!("ExecuteVacuum handled in run_db_thread loop")
        }
        DbWriteOp::ApplyReconciliation { .. } => {
            unreachable!("ApplyReconciliation handled in run_db_thread loop")
        }
        DbWriteOp::TruncatePackingTables => {
            with_retry("truncate_packing_tables", "all", || {
                packing_ops::execute_truncate_packing_tables(db)
            });
        }

        DbWriteOp::WritePackingManifest { rows } => {
            with_retry("write_packing_manifest", "batch", || {
                packing_ops::execute_write_packing_manifest(db, rows)
            });
        }

        DbWriteOp::WritePackingScores { rows } => {
            with_retry("write_packing_scores", "batch", || {
                packing_ops::execute_write_packing_scores(db, rows)
            });
        }

        DbWriteOp::WritePackingCandidates { rows } => {
            with_retry("write_packing_candidates", "batch", || {
                packing_ops::execute_write_packing_candidates(db, rows)
            });
        }

        DbWriteOp::WritePendingAcoustIdSubmissions { rows } => {
            with_retry("write_pending_acoustid_submissions", "batch", || {
                packing_ops::execute_write_pending_acoustid_submissions(db, rows)
            });
        }

        DbWriteOp::Shutdown => unreachable!("Shutdown handled in run_db_thread loop"),
    }
}
