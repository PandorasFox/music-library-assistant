//! DB thread executor — processes `DbWriteOp` messages on the write connection.
//!
//! Contains the main execution loop, retry logic, signal clearing helpers,
//! and all `execute_*` functions that perform the actual SQL operations.

mod external_ops;
mod index_ops;
mod packing_ops;

use rusqlite::params;

use crate::db::Database;
use crate::meta::signals::registry;

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


// ============================================================================
// Dirty / Library Operations
// ============================================================================

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

/// Execute ClearAllDirtyInodes: remove all dirty flags for a computation type.
/// Used by bulk computations that consume all dirty flags at once.
fn execute_clear_all_dirty_inodes(
    db: &Database,
    computation_type: &str,
) -> anyhow::Result<()> {
    db.conn().execute(
        "DELETE FROM dirty_inodes WHERE computation_type = ?1",
        params![computation_type],
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

    // Wrap both writes in one transaction — without this each `execute` call
    // auto-commits separately, costing one fsync per statement. Single
    // transaction = single fsync, which moved this op from ~330 ms outliers
    // down to the typical 1–2 ms range during library-scan bursts.
    let tx = db.conn().unchecked_transaction()?;

    tx.execute(
        "INSERT INTO inodes (inode, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
         VALUES (?1, 0, ?2, ?3, ?4, ?5)
         ON CONFLICT(inode) DO UPDATE SET
             is_dir = excluded.is_dir,
             mtime_secs = excluded.mtime_secs,
             mtime_nanos = excluded.mtime_nanos,
             file_size = excluded.file_size,
             scanned_at = excluded.scanned_at",
        params![inode, mtime_secs, mtime_nanos, file_size, scanned_at],
    )?;

    tx.execute(
        "INSERT OR IGNORE INTO inode_paths (inode, zone, path) VALUES (?1, 'library', ?2)",
        params![inode, stored_path],
    )?;

    tx.commit()?;
    Ok(())
}

/// Execute DeleteLibraryFile: remove a stale library file from the files table.
fn execute_delete_library_file(db: &Database, stored_path: &str) -> anyhow::Result<()> {
    // Wrap the find+delete+orphan-cleanup sequence in one transaction —
    // up to four statements that were previously auto-committing individually.
    let tx = db.conn().unchecked_transaction()?;

    // Find the inode for this library path before deleting, so we can clean
    // up the inode row if no paths remain.
    let inode_opt: Option<i64> = tx
        .query_row(
            "SELECT inode FROM inode_paths WHERE zone = 'library' AND path = ?1",
            params![stored_path],
            |row| row.get(0),
        )
        .ok();

    tx.execute(
        "DELETE FROM inode_paths WHERE zone = 'library' AND path = ?1",
        params![stored_path],
    )?;

    if let Some(inode) = inode_opt {
        let remaining: i64 = tx.query_row(
            "SELECT COUNT(*) FROM inode_paths WHERE inode = ?1",
            params![inode],
            |row| row.get(0),
        )?;
        if remaining == 0 {
            tx.execute("DELETE FROM inodes WHERE inode = ?1", params![inode])?;
        }
    }

    tx.commit()?;
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
            registry::clear_all_corpus_signals(db.conn(), *inode);
        }
        DbWriteOp::ClearMutableCorpusSignals { inode } => {
            registry::clear_mutable_corpus_signals(db.conn(), *inode);
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
            // Wrap the whole batch in a single transaction so we get one fsync
            // for the lot instead of one per signal. Per-signal failures are
            // logged but don't abort the batch — matches prior best-effort
            // semantics. SQLite WAL + synchronous=NORMAL fsyncs only on COMMIT,
            // so this collapses N fsyncs into 1 for batches of any size.
            with_retry("write_typed_signal_batch_tx", "batch", || {
                let tx = db.conn().unchecked_transaction()?;
                for signal in signals {
                    if let Err(e) = signal.clone().insert(&tx) {
                        crate::logging::log_error(format!(
                            "[DB_THREAD] write_typed_signal_batch item failed: {}",
                            e
                        ));
                    }
                }
                tx.commit()?;
                Ok(())
            });
        }
        DbWriteOp::UpdateFileMtime {
            zone,
            inode,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("update_file_mtime", zone, || {
                // mtime is now inode-level. The zone parameter is preserved in
                // the API but no longer used in the WHERE clause.
                let rows_affected = db.conn()
                    .execute(
                        "UPDATE inodes SET mtime_secs = ?1, mtime_nanos = ?2 WHERE inode = ?3",
                        params![mtime_secs, mtime_nanos, inode],
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
        } => {
            with_retry("index_audio_file", path, || {
                index_ops::execute_index_audio_file(db, path, file_data, audio_data, tags)
            });
        }

        DbWriteOp::DropFromIndex { inode, zone } => {
            with_retry("drop_from_index", zone, || {
                index_ops::execute_drop_from_index(db, *inode, zone)
            });
        }

        DbWriteOp::SetIndexTrackTags {
            inode,
            path,
            tags,
            tag_table,
        } => {
            with_retry("set_index_track_tags", path, || {
                index_ops::execute_set_index_track_tags(db, *inode, tags, tag_table)
            });
        }

        DbWriteOp::ApplyIndexTagOps {
            inode,
            path,
            ops,
            tag_table,
        } => {
            with_retry("apply_index_tag_ops", path, || {
                index_ops::execute_apply_index_tag_ops(db, *inode, ops, tag_table)
            });
        }

        DbWriteOp::UpdateTrackPathWithMetadata {
            old_path,
            old_inode,
            zone,
            new_path,
            new_inode,
            new_file_size,
            new_file_type,
        } => {
            with_retry("update_track_path_with_metadata", old_path, || {
                index_ops::execute_update_track_path_with_metadata(
                    db,
                    *old_inode,
                    zone,
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

        DbWriteOp::SetNeedsDiskFlush { inode, value } => {
            with_retry("set_needs_disk_flush", &inode.to_string(), || {
                index_ops::execute_set_needs_disk_flush(db, *inode, *value)
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

        DbWriteOp::ClearAllDirtyInodes { computation_type } => {
            with_retry("clear_all_dirty_inodes", &computation_type, || {
                execute_clear_all_dirty_inodes(db, &computation_type)
            });
        }

        DbWriteOp::InsertExternalMatch {
            inode,
            fingerprint,
            source,
            recording_id,
            confidence,
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

        DbWriteOp::UpsertCaaReleaseCache {
            release_id,
            status,
            response_json,
            image_count,
            fetched_at,
        } => {
            with_retry("upsert_caa_release_cache", release_id, || {
                external_ops::execute_upsert_caa_release_cache(
                    db,
                    release_id,
                    status,
                    response_json.as_deref(),
                    *image_count,
                    *fetched_at,
                )
            });
        }

        DbWriteOp::UpsertDeezerIsrcCache {
            isrc,
            status,
            deezer_album_id,
            cover_url,
            response_json,
            fetched_at,
        } => {
            with_retry("upsert_deezer_isrc_cache", isrc, || {
                external_ops::execute_upsert_deezer_isrc_cache(
                    db,
                    isrc,
                    status,
                    *deezer_album_id,
                    cover_url.as_deref(),
                    response_json.as_deref(),
                    *fetched_at,
                )
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

        DbWriteOp::DeletePackingDataForRelease { release_id } => {
            with_retry("delete_packing_data_for_release", &release_id, || {
                packing_ops::execute_delete_packing_data_for_release(db, &release_id)
            });
        }

        DbWriteOp::SetCacheSize { .. } => unreachable!("SetCacheSize handled in run_db_thread loop"),
        DbWriteOp::Shutdown => unreachable!("Shutdown handled in run_db_thread loop"),
    }
}
