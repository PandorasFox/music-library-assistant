//! DB thread executor — processes `DbWriteOp` messages on the write connection.
//!
//! Contains the main execution loop, retry logic, signal clearing helpers,
//! and all `execute_*` functions that perform the actual SQL operations.

use rusqlite::{params, OptionalExtension};

use crate::corpus::tags::TagSet;
use crate::db::Database;
use crate::meta::signals::data::*;
use crate::meta::signals::store::CorpusSignalStore;

use super::types::*;
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
                execute_index_audio_file(db, path, file_data, audio_data, tags, session_id)
            });
        }

        DbWriteOp::DropFromIndex { path } => {
            with_retry("drop_from_index", path, || {
                execute_drop_from_index(db, path)
            });
        }

        DbWriteOp::SetIndexTrackTags {
            path,
            tags,
            tag_table,
            session_id,
        } => {
            with_retry("set_index_track_tags", path, || {
                execute_set_index_track_tags(db, path, tags, tag_table, session_id)
            });
        }

        DbWriteOp::ApplyIndexTagOps {
            path,
            ops,
            tag_table,
            session_id,
        } => {
            with_retry("apply_index_tag_ops", path, || {
                execute_apply_index_tag_ops(db, path, ops, tag_table, session_id)
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
                execute_update_track_path_with_metadata(
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
                execute_upsert_file_entry(db, path, zone, file_entry)
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
                execute_index_directory(db, path, zone, *inode, *mtime_secs, *mtime_nanos)
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
                execute_index_image_file(
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
                execute_upsert_image_info(db, *inode, format, *width, *height, role)
            });
        }

        DbWriteOp::ClearTagMismatchesForTrack { path } => {
            with_retry("clear_tag_mismatches_for_track", path, || {
                execute_clear_tag_mismatches_for_track(db, path)
            });
        }

        DbWriteOp::SetNeedsDiskFlush { path, value } => {
            with_retry("set_needs_disk_flush", path, || {
                execute_set_needs_disk_flush(db, path, *value)
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
                execute_insert_external_match(
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
                execute_insert_external_no_match(db, fingerprint, *source, *queried_at)
            });
        }

        DbWriteOp::UpsertExternalRetry {
            inode,
            fingerprint,
            source,
            error,
        } => {
            with_retry("upsert_external_retry", error, || {
                execute_upsert_external_retry(db, *inode, fingerprint, *source, error)
            });
        }

        DbWriteOp::DeleteExternalRetry { inode, source } => {
            with_retry("delete_external_retry", &inode.to_string(), || {
                execute_delete_external_retry(db, *inode, *source)
            });
        }

        DbWriteOp::DropExternalMatch { inode } => {
            with_retry("drop_external_match", &inode.to_string(), || {
                execute_drop_external_match(db, *inode)
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
                execute_upsert_mb_cache(db, table, id_col, id, raw_json, *fetched_at)
            });
        }

        DbWriteOp::InsertMbKnownEntity {
            mbid,
            entity_type,
            discovered_from,
            discovered_at,
        } => {
            with_retry("insert_mb_known_entity", mbid, || {
                execute_insert_mb_known_entity(
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
                db.conn().execute_batch(
                    "DELETE FROM release_packing_manifest; DELETE FROM release_packing_scores; DELETE FROM release_packing_candidates; DELETE FROM pending_acoustid_submissions; DELETE FROM signal_packed_release;"
                )?;
                Ok(())
            });
        }

        DbWriteOp::WritePackingManifest { rows } => {
            with_retry("write_packing_manifest", "batch", || {
                let mut stmt = db.conn().prepare(
                    "INSERT OR REPLACE INTO release_packing_manifest (release_id, total_tracks, release_title, release_artist, media_count) VALUES (?1, ?2, ?3, ?4, ?5)"
                )?;
                for (release_id, total_tracks, title, artist, media_count) in rows {
                    stmt.execute(rusqlite::params![release_id, total_tracks, title, artist, media_count])?;
                }
                Ok(())
            });
        }

        DbWriteOp::WritePackingScores { rows } => {
            with_retry("write_packing_scores", "batch", || {
                let mut stmt = db.conn().prepare(
                    "INSERT OR REPLACE INTO release_packing_scores \
                     (release_id, inode, recording_id, medium_pos, track_pos, track_title, medium_format, track_number, score, score_breakdown, is_optimal, match_method, fingerprint_hex, raw_duration_ms) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)"
                )?;
                for row in rows {
                    stmt.execute(rusqlite::params![
                        row.release_id,
                        row.inode,
                        row.recording_id,
                        row.medium_pos,
                        row.track_pos,
                        row.track_title,
                        row.medium_format,
                        row.track_number,
                        row.score,
                        row.score_breakdown,
                        row.is_optimal as i32,
                        row.match_method,
                        row.fingerprint_hex,
                        row.raw_duration_ms,
                    ])?;
                }
                Ok(())
            });
        }

        DbWriteOp::WritePackingCandidates { rows } => {
            with_retry("write_packing_candidates", "batch", || {
                let mut stmt = db.conn().prepare(
                    "INSERT OR REPLACE INTO release_packing_candidates \
                     (release_id, inode, recording_id, confidence, path, parent_dir, duration_ms, \
                      tag_title, tag_artist, tag_album, tag_tracknumber, dir_file_count) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
                )?;
                for row in rows {
                    stmt.execute(rusqlite::params![
                        row.release_id,
                        row.inode,
                        row.recording_id,
                        row.confidence,
                        row.path,
                        row.parent_dir,
                        row.duration_ms,
                        row.tag_title,
                        row.tag_artist,
                        row.tag_album,
                        row.tag_tracknumber,
                        row.dir_file_count,
                    ])?;
                }
                Ok(())
            });
        }

        DbWriteOp::WritePendingAcoustIdSubmissions { rows } => {
            with_retry("write_pending_acoustid_submissions", "batch", || {
                let mut stmt = db.conn().prepare(
                    "INSERT OR REPLACE INTO pending_acoustid_submissions \
                     (fingerprint, recording_id, duration_ms, source) \
                     VALUES (?1, ?2, ?3, ?4)",
                )?;
                for row in rows {
                    stmt.execute(rusqlite::params![
                        row.fingerprint,
                        row.recording_id,
                        row.duration_ms,
                        row.source,
                    ])?;
                }
                Ok(())
            });
        }

        DbWriteOp::Shutdown => unreachable!("Shutdown handled in run_db_thread loop"),
    }
}

// ============================================================================
// Index Operation Helpers (internal to db_thread)
// ============================================================================

/// Current time as Unix epoch seconds (i64).
fn current_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Increment the tags_version counter for an inode.
/// Called whenever tags are modified (not on initial indexing).
fn increment_tags_version(conn: &rusqlite::Connection, inode: i64) -> anyhow::Result<()> {
    conn.execute(
        "UPDATE audio_info SET tags_version = tags_version + 1 WHERE inode = ?1",
        params![inode],
    )?;
    Ok(())
}

/// Write tag edit history entries for changed tags.
fn write_tag_edit_history(
    conn: &rusqlite::Connection,
    inode: i64,
    changes: &[(String, Option<String>, Option<String>)], // (field_name, old_value, new_value)
    session_id: &str,
) -> anyhow::Result<()> {

    for (field_name, old_value, new_value) in changes {
        conn.execute(
            "INSERT INTO tag_edit_history (inode, field_name, old_value, new_value, session_id) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![inode, field_name, old_value, new_value, session_id],
        )?;
    }

    Ok(())
}

/// Convert fingerprint Vec<u32> to BLOB bytes (little-endian).
fn fingerprint_to_blob(fp: &[u32]) -> Vec<u8> {
    fp.iter().flat_map(|n| n.to_le_bytes()).collect()
}

/// Result of applying a TagSet to an inode.
///
/// Contains the changes made for history writing.
struct TagMutationResult {
    /// Tags removed: (field_name, old_value)
    removed: Vec<(String, String)>,
    /// Tags added: (field_name, new_value)
    added: Vec<(String, String)>,
}

impl TagMutationResult {
    /// Whether any changes were made.
    fn has_changes(&self) -> bool {
        !self.removed.is_empty() || !self.added.is_empty()
    }

    /// Convert to history entries: (field_name, old_value, new_value).
    fn to_history_entries(&self) -> Vec<(String, Option<String>, Option<String>)> {
        let mut entries = Vec::with_capacity(self.removed.len() + self.added.len());
        for (field, value) in &self.removed {
            entries.push((field.clone(), Some(value.clone()), None));
        }
        for (field, value) in &self.added {
            entries.push((field.clone(), None, Some(value.clone())));
        }
        entries
    }
}

/// Apply a TagSet to an inode, computing and executing the minimal diff.
///
/// Uses TagSet::diff() to determine what changed:
/// - DELETEs tags in existing but not in desired
/// - INSERTs tags in desired but not in existing
///
/// Returns the changes made for history writing. Does NOT:
/// - Write history (caller decides if this is an edit vs discovery)
/// - Increment tags_version (caller decides)
/// - Mark inode dirty (caller decides)
///
/// Caller must provide a transaction for atomicity.
fn apply_tagset_to_inode(
    tx: &rusqlite::Transaction,
    inode: i64,
    new_tags: &TagSet,
    tag_table: &str,
) -> anyhow::Result<TagMutationResult> {

    // Query existing tags from DB and build a TagSet
    let mut stmt = tx.prepare(&format!(
        "SELECT tag_name, tag_value FROM {} WHERE inode = ?1",
        tag_table
    ))?;
    let existing_pairs: Vec<(String, String)> = stmt
        .query_map(params![inode], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .filter_map(|r| r.ok())
        .collect();
    drop(stmt);

    let existing = TagSet::new(existing_pairs);

    // Use TagSet::diff() to compute changes
    // existing.diff(new_tags) gives:
    //   only_left = in existing but not in new_tags → DELETE these
    //   only_right = in new_tags but not in existing → INSERT these
    let diff = existing.diff(new_tags);

    // Collect for result before consuming
    let removed: Vec<(String, String)> = diff
        .only_left
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let added: Vec<(String, String)> = diff
        .only_right
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();

    // DELETE tags that should be removed
    for (name, value) in &removed {
        tx.execute(
            &format!(
                "DELETE FROM {} WHERE inode = ?1 AND tag_name = ?2 AND tag_value = ?3",
                tag_table
            ),
            params![inode, name, value],
        )?;
    }

    // INSERT tags that are new
    for (name, value) in &added {
        tx.execute(
            &format!(
                "INSERT INTO {} (inode, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                tag_table
            ),
            params![inode, name, value],
        )?;
    }

    Ok(TagMutationResult { removed, added })
}

/// Get inode by path from files table. Returns None if file doesn't exist.
fn get_inode_by_path(db: &Database, path: &str) -> anyhow::Result<Option<i64>> {
    db.conn()
        .query_row(
            "SELECT inode FROM files WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e: rusqlite::Error| anyhow::anyhow!(e))
}

/// Execute IndexAudioFile: insert/replace files + audio_info + corpus_tags.
///
/// All operations wrapped in a single transaction for atomicity.
/// Uses apply_tagset_to_inode for atomic diff-based tag replacement.
/// Writes tag_edit_history for discovered tags (old_value=None, new_value=tag).
fn execute_index_audio_file(
    db: &Database,
    path: &str,
    file_data: &FileData,
    audio_data: &AudioData,
    tags: &TagSet,
    session_id: &str,
) -> anyhow::Result<()> {
    let scanned_at = current_unix_secs();

    // Determine which tag table to use based on zone
    let tag_table = if file_data.zone == "inbox" {
        "inbox_tags"
    } else {
        "corpus_tags"
    };

    // Wrap all operations in a single transaction
    let tx = db.conn().unchecked_transaction()?;

    // Insert or replace files row
    tx.execute(
        r#"
        INSERT OR REPLACE INTO files
        (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            file_data.inode,
            &file_data.zone,
            path,
            0i32, // is_dir = false for audio files
            file_data.mtime_secs,
            file_data.mtime_nanos,
            file_data.file_size,
            scanned_at,
        ],
    )?;

    // Convert fingerprint to BLOB if present
    let fp_blob: Option<Vec<u8>> = audio_data
        .fingerprint
        .as_ref()
        .map(|fp| fingerprint_to_blob(fp));

    // Upsert audio_info row.
    // IMPORTANT: Must use ON CONFLICT DO UPDATE (not INSERT OR REPLACE) because
    // REPLACE triggers DELETE+INSERT which cascades to tag_edit_history via FK.
    tx.execute(
        r#"
        INSERT INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures,
         pic_format, pic_width, pic_height, pic_count, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 0)
        ON CONFLICT(inode) DO UPDATE SET
            file_type = excluded.file_type,
            duration_ms = excluded.duration_ms,
            bitrate_kbps = excluded.bitrate_kbps,
            sample_rate = excluded.sample_rate,
            fingerprint = excluded.fingerprint,
            has_pictures = excluded.has_pictures,
            pic_format = excluded.pic_format,
            pic_width = excluded.pic_width,
            pic_height = excluded.pic_height,
            pic_count = excluded.pic_count
        "#,
        params![
            file_data.inode,
            &audio_data.file_type,
            audio_data.duration_ms,
            audio_data.bitrate_kbps,
            audio_data.sample_rate,
            &fp_blob,
            audio_data.has_pictures as i32,
            &audio_data.pic_format,
            audio_data.pic_width,
            audio_data.pic_height,
            audio_data.pic_count,
        ],
    )?;

    // Apply tags using the unified helper
    let result = apply_tagset_to_inode(&tx, file_data.inode, tags, tag_table)?;

    // Write history for discovered/changed tags
    if result.has_changes() {
        let changes = result.to_history_entries();
        write_tag_edit_history(&tx, file_data.inode, &changes, session_id)?;
    }

    tx.commit()?;
    Ok(())
}

/// Execute DropFromIndex: delete file entry and cascades.
/// audio_info and tags cascade via foreign keys.
fn execute_drop_from_index(db: &Database, path: &str) -> anyhow::Result<()> {

    // Get inode first
    let inode = match get_inode_by_path(db, path)? {
        Some(id) => id,
        None => return Ok(()), // File doesn't exist, nothing to do
    };

    // Delete tag_edit_history first (plain FK without CASCADE)
    db.conn().execute(
        "DELETE FROM tag_edit_history WHERE inode = ?1",
        params![inode],
    )?;

    // Delete the file entry (audio_info, corpus_tags/inbox_tags cascade automatically)
    db.conn()
        .execute("DELETE FROM files WHERE path = ?1", params![path])?;

    // If no other paths reference this inode, clean up audio_info
    // (FK CASCADE should handle this, but be explicit)
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM files WHERE inode = ?1",
        params![inode],
        |row| row.get(0),
    )?;
    if count == 0 {
        db.conn()
            .execute("DELETE FROM audio_info WHERE inode = ?1", params![inode])?;
    }

    // Clear all corpus signals for this inode from typed tables
    typed_clear_corpus_signals(db, inode, true);

    Ok(())
}

/// Execute SetIndexTrackTags: replace all tags for a file (corpus_tags).
///
/// Uses apply_tagset_to_inode for atomic diff-based replacement.
/// Writes tag edit history for all changes (this IS an edit, not discovery).
///
/// Used by AssimilateDiskTagsToDb when accepting disk changes.
fn execute_set_index_track_tags(
    db: &Database,
    path: &str,
    tags: &TagSet,
    tag_table: &str,
    session_id: &str,
) -> anyhow::Result<()> {
    let inode =
        get_inode_by_path(db, path)?.ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

    let tx = db.conn().unchecked_transaction()?;

    // Apply tags using the unified helper
    let result = apply_tagset_to_inode(&tx, inode, tags, tag_table)?;

    if result.has_changes() {
        // Increment tags_version using the helper
        increment_tags_version(&tx, inode)?;

        // Write tag edit history using the caller-provided session identifier
        let changes = result.to_history_entries();
        write_tag_edit_history(&tx, inode, &changes, session_id)?;
    }

    tx.commit()?;
    Ok(())
}

/// Execute ApplyIndexTagOps: apply incremental tag operations directly.
///
/// TagOps map directly to SQL operations:
/// - add (old=None, new=Some) → INSERT OR IGNORE (idempotent)
/// - drop (old=Some, new=None) → DELETE with exact match
/// - replace (old=Some, new=Some) → UPDATE with exact match
///
/// All operations run in a single transaction for atomicity.
fn execute_apply_index_tag_ops(
    db: &Database,
    path: &str,
    ops: &[crate::meta::mutations::TagOp],
    tag_table: &str,
    session_id: &str,
) -> anyhow::Result<()> {

    let inode =
        get_inode_by_path(db, path)?.ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

    // Filter to non-nop operations
    let effective_ops: Vec<_> = ops.iter().filter(|op| !op.is_nop()).collect();
    if effective_ops.is_empty() {
        return Ok(());
    }

    let tx = db.conn().unchecked_transaction()?;

    // Collect history entries while applying operations
    let mut history_entries: Vec<(String, Option<String>, Option<String>)> = Vec::new();

    for op in &effective_ops {
        let tag_name = op.tag_name.to_uppercase();

        match (&op.old_value, &op.new_value) {
            (Some(old), Some(new)) => {
                // Replace: UPDATE in place
                // Use UPPER() for tag_name match - tag tables may store original case from
                // audio files but ops are normalized to uppercase.
                tx.execute(
                    &format!("UPDATE {} SET tag_value = ?1 WHERE inode = ?2 AND UPPER(tag_name) = ?3 AND tag_value = ?4", tag_table),
                    params![new, inode, &tag_name, old],
                )?;
            }
            (Some(old), None) => {
                // Drop: DELETE with exact match
                tx.execute(
                    &format!("DELETE FROM {} WHERE inode = ?1 AND UPPER(tag_name) = ?2 AND tag_value = ?3", tag_table),
                    params![inode, &tag_name, old],
                )?;
            }
            (None, Some(new)) => {
                // Add: INSERT OR IGNORE (idempotent - won't fail if already exists)
                tx.execute(
                    &format!(
                        "INSERT OR IGNORE INTO {} (inode, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                        tag_table
                    ),
                    params![inode, &tag_name, new],
                )?;
            }
            (None, None) => {
                // No-op - should not reach here due to filter
            }
        }

        // Collect history entry for this operation
        history_entries.push((tag_name, op.old_value.clone(), op.new_value.clone()));
    }

    // Write history entries using the caller-provided session identifier
    write_tag_edit_history(&tx, inode, &history_entries, session_id)?;

    // Increment tags_version using the helper
    increment_tags_version(&tx, inode)?;

    tx.commit()?;
    Ok(())
}

/// Execute UpdateTrackPathWithMetadata: update path and file metadata for transcoded file.
/// Updates files table path/inode/file_size, and audio_info file_type.
///
/// All operations wrapped in a single transaction for atomicity.
fn execute_update_track_path_with_metadata(
    db: &Database,
    old_path: &str,
    new_path: &str,
    new_inode: i64,
    new_file_size: i64,
    new_file_type: &str,
) -> anyhow::Result<()> {

    // Get old inode
    let old_inode = get_inode_by_path(db, old_path)?
        .ok_or_else(|| anyhow::anyhow!("File not found at old path: {}", old_path))?;

    let scanned_at = current_unix_secs();

    // Wrap all operations in a single transaction
    let tx = db.conn().unchecked_transaction()?;

    // Update files table
    let rows_updated = tx.execute(
        "UPDATE files SET path = ?1, inode = ?2, file_size = ?3, scanned_at = ?4 WHERE path = ?5",
        params![new_path, new_inode, new_file_size, scanned_at, old_path],
    )?;

    if rows_updated == 0 {
        anyhow::bail!("File not found at old path: {}", old_path);
    }

    // Get old audio_info to copy to new inode
    type AudioInfoRow = (Option<i64>, Option<i32>, Option<i32>, Option<Vec<u8>>, i32);
    let (duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures): AudioInfoRow =
        tx.query_row(
            "SELECT duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures FROM audio_info WHERE inode = ?1",
            params![old_inode],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get::<_, i32>(4).unwrap_or(0))),
        )?;

    // Upsert new audio_info with new file_type.
    // Must use ON CONFLICT DO UPDATE (not REPLACE) to avoid CASCADE on tag_edit_history.
    tx.execute(
        r#"
        INSERT INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
        ON CONFLICT(inode) DO UPDATE SET
            file_type = excluded.file_type,
            duration_ms = excluded.duration_ms,
            bitrate_kbps = excluded.bitrate_kbps,
            sample_rate = excluded.sample_rate,
            fingerprint = excluded.fingerprint,
            has_pictures = excluded.has_pictures
        "#,
        params![new_inode, new_file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures],
    )?;

    // Copy tags to new inode
    tx.execute(
        "INSERT OR IGNORE INTO corpus_tags SELECT ?1, tag_name, tag_value FROM corpus_tags WHERE inode = ?2",
        params![new_inode, old_inode],
    )?;

    // Clean up old inode if orphaned
    if old_inode != new_inode {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM files WHERE inode = ?1",
            params![old_inode],
            |row| row.get(0),
        )?;
        if count == 0 {
            tx.execute(
                "DELETE FROM audio_info WHERE inode = ?1",
                params![old_inode],
            )?;
            tx.execute(
                "DELETE FROM corpus_tags WHERE inode = ?1",
                params![old_inode],
            )?;
        }
    }

    tx.commit()?;
    Ok(())
}

/// Execute UpsertFileEntry: insert or update file entry in files table.
fn execute_upsert_file_entry(
    db: &Database,
    path: &str,
    zone: &str,
    file_entry: &FileEntryData,
) -> anyhow::Result<()> {
    let scanned_at = current_unix_secs();

    // Upsert into files table
    db.conn().execute(
        r#"
        INSERT INTO files (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7)
        ON CONFLICT(inode, zone, path) DO UPDATE SET
            mtime_secs = excluded.mtime_secs,
            mtime_nanos = excluded.mtime_nanos,
            file_size = excluded.file_size,
            scanned_at = excluded.scanned_at
        "#,
        params![
            file_entry.inode,
            zone,
            path,
            file_entry.mtime_secs,
            file_entry.mtime_nanos,
            file_entry.file_size,
            scanned_at,
        ],
    )?;

    Ok(())
}

/// Execute ClearTagMismatchesForTrack: clear all mismatches for a file.
/// NOTE: tag_mismatches table is dropped in new schema. Tag conflicts are
/// now handled via OOB signals with typed mismatch data in bincode BLOBs.
/// This is a no-op placeholder until callers are updated.
fn execute_clear_tag_mismatches_for_track(_db: &Database, _path: &str) -> anyhow::Result<()> {
    // TODO: Clear OOB signals for this path when tag conflicts are fully signal-based
    Ok(())
}

/// Execute IndexDirectory: insert directory entry in files table.
/// Used during corpus scanning to track directory entries.
fn execute_index_directory(
    db: &Database,
    path: &str,
    zone: &str,
    inode: i64,
    mtime_secs: i64,
    mtime_nanos: i64,
) -> anyhow::Result<()> {
    let scanned_at = current_unix_secs();

    // Insert or replace directory entry
    db.conn().execute(
        r#"
        INSERT OR REPLACE INTO files
        (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, 1, ?4, ?5, 0, ?6)
        "#,
        params![inode, zone, path, mtime_secs, mtime_nanos, scanned_at,],
    )?;

    Ok(())
}

/// Execute IndexImageFile: insert image file entry in files table.
fn execute_index_image_file(
    db: &Database,
    path: &str,
    zone: &str,
    inode: i64,
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: i64,
) -> anyhow::Result<()> {
    let scanned_at = current_unix_secs();

    db.conn().execute(
        r#"
        INSERT INTO files (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7)
        ON CONFLICT(inode, zone, path) DO UPDATE SET
            mtime_secs = excluded.mtime_secs,
            mtime_nanos = excluded.mtime_nanos,
            file_size = excluded.file_size,
            scanned_at = excluded.scanned_at
        "#,
        params![inode, zone, path, mtime_secs, mtime_nanos, file_size, scanned_at],
    )?;

    Ok(())
}

/// Execute UpsertImageInfo: insert or update image metadata.
fn execute_upsert_image_info(
    db: &Database,
    inode: i64,
    format: &str,
    width: u32,
    height: u32,
    role: &str,
) -> anyhow::Result<()> {

    db.conn().execute(
        "INSERT OR REPLACE INTO image_info (inode, format, width, height, role)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![inode, format, width, height, role],
    )?;

    Ok(())
}

/// Execute SetNeedsDiskFlush: update the needs_tag_flush flag on audio_info.
fn execute_set_needs_disk_flush(db: &Database, path: &str, value: bool) -> anyhow::Result<()> {

    // Get inode from path
    let inode = match get_inode_by_path(db, path)? {
        Some(id) => id,
        None => {
            crate::logging::log_error(format!(
                "[DB_THREAD] set_needs_disk_flush: no file found for path={}",
                path
            ));
            return Ok(());
        }
    };

    let rows_updated = db.conn().execute(
        "UPDATE audio_info SET needs_tag_flush = ?1 WHERE inode = ?2",
        params![value as i32, inode],
    )?;

    if rows_updated == 0 {
        crate::logging::log_error(format!(
            "[DB_THREAD] set_needs_disk_flush: no audio_info found for inode={}",
            inode
        ));
    }

    Ok(())
}

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
    let now = current_unix_secs();
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
    let scanned_at = current_unix_secs();

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

/// Execute InsertExternalMatch: insert a match from an external API.
#[allow(clippy::too_many_arguments)] // args map 1:1 to SQL columns
fn execute_insert_external_match(
    db: &Database,
    inode: i64,
    fingerprint: &[u8],
    source: i64,
    recording_id: &str,
    confidence: f64,
    raw_response: Option<&[u8]>,
    fetched_at: i64,
) -> anyhow::Result<()> {

    db.conn().execute(
        r#"INSERT OR REPLACE INTO external_matches
           (inode, fingerprint, source, recording_id, confidence, raw_response, fetched_at)
           VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)"#,
        params![
            inode,
            fingerprint,
            source,
            recording_id,
            confidence,
            raw_response,
            fetched_at
        ],
    )?;

    Ok(())
}

/// Execute InsertExternalNoMatch: record that a fingerprint had no results.
fn execute_insert_external_no_match(
    db: &Database,
    fingerprint: &[u8],
    source: i64,
    queried_at: i64,
) -> anyhow::Result<()> {

    db.conn().execute(
        r#"INSERT OR REPLACE INTO external_no_match
           (fingerprint, source, queried_at)
           VALUES (?1, ?2, ?3)"#,
        params![fingerprint, source, queried_at],
    )?;

    Ok(())
}

/// Execute UpsertExternalRetry: record a failed lookup for retry.
fn execute_upsert_external_retry(
    db: &Database,
    inode: i64,
    fingerprint: &[u8],
    source: i64,
    error: &str,
) -> anyhow::Result<()> {
    let now = current_unix_secs();

    db.conn().execute(
        r#"INSERT INTO external_retry (inode, fingerprint, source, failed_at, error, retry_count)
           VALUES (?1, ?2, ?3, ?4, ?5, 1)
           ON CONFLICT(inode, source) DO UPDATE SET
               failed_at = excluded.failed_at,
               error = excluded.error,
               retry_count = retry_count + 1"#,
        params![inode, fingerprint, source, now, error],
    )?;

    Ok(())
}

/// Execute DeleteExternalRetry: remove retry entry after successful lookup.
fn execute_delete_external_retry(db: &Database, inode: i64, source: i64) -> anyhow::Result<()> {

    db.conn().execute(
        "DELETE FROM external_retry WHERE inode = ?1 AND source = ?2",
        params![inode, source],
    )?;

    Ok(())
}

fn execute_drop_external_match(db: &Database, inode: i64) -> anyhow::Result<()> {

    db.conn().execute(
        "DELETE FROM external_matches WHERE inode = ?1",
        params![inode],
    )?;

    Ok(())
}

/// Execute an MB cache upsert for any entity type.
fn execute_upsert_mb_cache(
    db: &Database,
    table: &str,
    id_col: &str,
    id: &str,
    raw_json: &[u8],
    fetched_at: i64,
) -> anyhow::Result<()> {

    let sql = format!(
        "INSERT OR REPLACE INTO {table} ({id_col}, raw_json, fetched_at) VALUES (?1, ?2, ?3)"
    );
    db.conn().execute(&sql, params![id, raw_json, fetched_at])?;
    Ok(())
}

/// Execute InsertMbKnownEntity: persist a discovered MB entity for resumable fetching.
fn execute_insert_mb_known_entity(
    db: &Database,
    mbid: &str,
    entity_type: &str,
    discovered_from: Option<&str>,
    discovered_at: i64,
) -> anyhow::Result<()> {

    db.conn().execute(
        r#"INSERT OR IGNORE INTO mb_known_entities
           (mbid, entity_type, discovered_from, discovered_at)
           VALUES (?1, ?2, ?3, ?4)"#,
        params![mbid, entity_type, discovered_from, discovered_at],
    )?;

    Ok(())
}
