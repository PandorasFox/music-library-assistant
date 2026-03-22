//! Index operations: audio file indexing, tag manipulation, file entry management.

use rusqlite::{params, OptionalExtension};

use crate::corpus::tags::TagSet;
use crate::db::Database;

use super::super::types::*;

// ============================================================================
// Shared Helpers
// ============================================================================

/// Current time as Unix epoch seconds (i64).
pub(super) fn current_unix_secs() -> i64 {
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

// ============================================================================
// Index Operation Functions
// ============================================================================

/// Execute IndexAudioFile: insert/replace files + audio_info + corpus_tags.
///
/// All operations wrapped in a single transaction for atomicity.
/// Uses apply_tagset_to_inode for atomic diff-based tag replacement.
/// Writes tag_edit_history for discovered tags (old_value=None, new_value=tag).
pub(super) fn execute_index_audio_file(
    db: &Database,
    path: &str,
    file_data: &FileData,
    audio_data: &AudioData,
    tags: &TagSet,
    session_id: &str,
) -> anyhow::Result<()> {
    let scanned_at = current_unix_secs();

    let tag_table = "corpus_tags";

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
pub(super) fn execute_drop_from_index(db: &Database, path: &str) -> anyhow::Result<()> {

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

    // Delete the file entry (audio_info, corpus_tags cascade automatically)
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
    crate::meta::signals::registry::clear_all_corpus_signals(db.conn(), inode);

    Ok(())
}

/// Execute SetIndexTrackTags: replace all tags for a file (corpus_tags).
///
/// Uses apply_tagset_to_inode for atomic diff-based replacement.
/// Writes tag edit history for all changes (this IS an edit, not discovery).
///
/// Used by AssimilateDiskTagsToDb when accepting disk changes.
pub(super) fn execute_set_index_track_tags(
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
pub(super) fn execute_apply_index_tag_ops(
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
pub(super) fn execute_update_track_path_with_metadata(
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
pub(super) fn execute_upsert_file_entry(
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

/// Execute IndexDirectory: insert directory entry in files table.
/// Used during corpus scanning to track directory entries.
pub(super) fn execute_index_directory(
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
pub(super) fn execute_index_image_file(
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
pub(super) fn execute_upsert_image_info(
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
pub(super) fn execute_set_needs_disk_flush(db: &Database, path: &str, value: bool) -> anyhow::Result<()> {

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

/// No-op: tag_mismatches table was dropped. Tag conflicts are now handled
/// via OOB signals (OutOfBandTagSyncSignal, OutOfBandTagConflictSignal, MtimeOnlyMismatchSignal).
pub(super) fn execute_clear_tag_mismatches_for_track(_db: &Database, _path: &str) -> anyhow::Result<()> {
    Ok(())
}
