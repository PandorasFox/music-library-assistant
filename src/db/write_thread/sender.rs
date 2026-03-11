//! `SignalWriteSender` — the public API for enqueuing write operations.
//!
//! All methods require appropriate witness types to ensure only authorized
//! execution contexts can enqueue writes.

use std::sync::atomic::Ordering;
use std::sync::mpsc::Sender;
use std::sync::Arc;

use crate::meta::computations::ComputationWitness;
use crate::witch::MutationExecutionWitness;

use super::types::*;
use super::SharedStats;
use super::DbWriteOp;

/// Sender for signal write operations.
///
/// Clone-able, thread-safe. All methods require `ComputationWitness` to ensure
/// only computation execution contexts can enqueue signal writes.
#[derive(Clone)]
pub struct SignalWriteSender {
    pub(super) tx: Sender<DbWriteOp>,
    pub(super) stats: Arc<SharedStats>,
}

impl SignalWriteSender {
    /// Update queue stats when enqueuing an operation.
    #[inline]
    pub(super) fn mark_enqueued(&self) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
    }

    // =========================================================================
    // Signal clear operations (generic, resolved to function pointers at send time)
    // =========================================================================

    /// Clear an inode-keyed corpus signal.
    ///
    /// The type parameter resolves to a concrete `clear_by_inode` function pointer
    /// at compile time via the `CorpusSignalStore` trait.
    pub fn clear_corpus_signal<S: crate::meta::signals::store::CorpusSignalStore>(
        &self,
        inode: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearCorpusSignalByInode {
            clear_fn: S::clear_by_inode,
            inode,
            label: S::TABLE_NAME,
        });
    }

    /// Clear all corpus signals for an inode.
    ///
    /// Used when dropping a file from the index to clear all associated signals.
    pub fn clear_all_corpus_signals(&self, inode: i64, _witness: &impl SignalWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearAllCorpusSignals { inode });
    }

    /// Clear mutable corpus signals for an inode (preserves CorruptFile, ShitFormat).
    ///
    /// Used post-mutation when the file still exists but its state changed.
    /// File-inherent signals (CorruptFile, ShitFormat) are preserved because
    /// they represent intrinsic file properties, not computed state.
    pub fn clear_mutable_corpus_signals(&self, inode: i64, _witness: &impl SignalWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearMutableCorpusSignals { inode });
    }

    // =========================================================================
    // Aggregate signal operations
    // =========================================================================

    /// Clear an aggregate signal by key.
    ///
    /// The type parameter resolves to a concrete `clear_by_key` function pointer
    /// at compile time via the `AggregateSignalStore` trait.
    pub fn clear_aggregate_signal<S: crate::meta::signals::store::AggregateSignalStore>(
        &self,
        key: &str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearAggregateSignalByKey {
            clear_fn: S::clear_by_key,
            key: key.to_string(),
            label: S::TABLE_NAME,
        });
    }

    /// Clear an aggregate signal by key using a pre-resolved function pointer.
    ///
    /// Used by `SignalToClear` where the signal type is determined at construction
    /// time and carried as a function pointer rather than a type parameter.
    pub fn clear_aggregate_signal_fn(
        &self,
        clear_fn: fn(&rusqlite::Connection, &str) -> rusqlite::Result<()>,
        key: &str,
        label: &'static str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearAggregateSignalByKey {
            clear_fn,
            key: key.to_string(),
            label,
        });
    }

    /// Clear all aggregate signals whose key starts with the given prefix.
    ///
    /// Used for bulk clearing like all LibraryLeftover signals for one library.
    pub fn clear_aggregate_by_key_prefix<S: crate::meta::signals::store::AggregateSignalStore>(
        &self,
        prefix: &str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearAggregateByKeyPrefix {
            clear_fn: S::clear_by_key_prefix,
            prefix: prefix.to_string(),
            label: S::TABLE_NAME,
        });
    }

    /// Clear all rows from a signal table (DELETE FROM).
    ///
    /// Used for bulk clear-then-write patterns where an entire pipeline
    /// re-emits all signals from scratch.
    pub fn clear_signal_table<S: crate::meta::signals::store::CorpusSignalStore>(
        &self,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearSignalTable {
            clear_fn: |conn| S::clear_all(conn),
            label: S::TABLE_NAME,
        });
    }

    /// Clear all rows from an aggregate signal table (DELETE FROM).
    ///
    /// Used for bulk clear-then-write patterns where an entire pipeline
    /// re-emits all signals from scratch.
    pub fn clear_aggregate_signal_table<S: crate::meta::signals::store::AggregateSignalStore>(
        &self,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearSignalTable {
            clear_fn: |conn| S::clear_all(conn),
            label: S::TABLE_NAME,
        });
    }

    /// Write a typed signal directly to its per-signal table.
    ///
    /// This is the typed-data path that bypasses JSON serialization.
    /// Computations construct the typed data struct and send it directly.
    pub fn write_typed_signal(
        &self,
        signal: crate::meta::signals::registry::TypedSignalWrite,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::WriteTypedSignal { signal });
    }

    /// Write a batch of typed signals in a single channel message.
    ///
    /// All signals are inserted in one transaction on the DB thread, reducing
    /// both channel overhead and SQLite transaction costs for bulk reconciliation.
    pub fn write_typed_signal_batch(
        &self,
        signals: Vec<crate::meta::signals::registry::TypedSignalWrite>,
        _witness: &impl SignalWitness,
    ) {
        if signals.is_empty() {
            return;
        }
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::WriteTypedSignalBatch { signals });
    }

    // =========================================================================
    // Library File Operations (Awakening phase - reconciliation)
    // =========================================================================

    /// Upsert a library file during reconciliation (new or changed).
    pub fn upsert_library_file(
        &self,
        stored_path: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpsertLibraryFile {
            stored_path: stored_path.to_string(),
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
        });
    }

    /// Delete a stale library file during reconciliation.
    pub fn delete_library_file(&self, stored_path: &str, _witness: &ComputationWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::DeleteLibraryFile {
            stored_path: stored_path.to_string(),
        });
    }

    // =========================================================================
    // Bulk Operations (Awake phase content analysis)
    // =========================================================================

    /// Update file mtime in files table (after OOB verification).
    /// Uses (zone, inode) as the unique key for reliable updates.
    pub fn update_file_mtime(
        &self,
        zone: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpdateFileMtime {
            zone: zone.to_string(),
            inode,
            mtime_secs,
            mtime_nanos,
        });
    }

    // =========================================================================
    // File/Audio Index Operations (Mutation execution)
    // =========================================================================

    /// Index an audio file (files + audio_info + corpus_tags).
    ///
    /// For corpus files, tags go to corpus_tags table.
    /// For inbox files, tags go to inbox_tags table.
    pub fn index_audio_file(
        &self,
        path: &str,
        file_data: FileData,
        audio_data: AudioData,
        tags: crate::corpus::tags::TagSet,
        session_id: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::IndexAudioFile {
            path: path.to_string(),
            file_data,
            audio_data,
            tags,
            session_id: session_id.to_string(),
        });
    }

    /// Drop a file from the index by path.
    pub fn drop_from_index(&self, path: &str, _witness: &MutationExecutionWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::DropFromIndex {
            path: path.to_string(),
        });
    }

    /// Set all tags for a track (replaces existing).
    ///
    /// Used by AssimilateDiskTagsToDb when accepting disk changes.
    /// For incremental tag edits (ApplyTagOps), use `apply_index_tag_ops` instead.
    pub fn set_index_track_tags(
        &self,
        path: &str,
        tags: crate::corpus::tags::TagSet,
        tag_table: &str,
        session_id: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::SetIndexTrackTags {
            path: path.to_string(),
            tags,
            tag_table: tag_table.to_string(),
            session_id: session_id.to_string(),
        });
    }

    /// Apply incremental tag operations directly.
    ///
    /// Used by ApplyTagOps for precise INSERT/DELETE operations without
    /// recomputing the diff. TagOps map directly to SQL operations:
    /// - add → INSERT OR IGNORE
    /// - drop → DELETE
    /// - replace → DELETE + INSERT
    pub fn apply_index_tag_ops(
        &self,
        path: &str,
        ops: Vec<crate::meta::mutations::TagOp>,
        tag_table: &str,
        session_id: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ApplyIndexTagOps {
            path: path.to_string(),
            ops,
            tag_table: tag_table.to_string(),
            session_id: session_id.to_string(),
        });
    }

    /// Update track path and file metadata atomically.
    /// Used when a file is transcoded/converted to a new format.
    pub fn update_track_path_with_metadata(
        &self,
        old_path: &str,
        new_path: &str,
        new_inode: i64,
        new_file_size: i64,
        new_file_type: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpdateTrackPathWithMetadata {
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
            new_inode,
            new_file_size,
            new_file_type: new_file_type.to_string(),
        });
    }

    /// Upsert file entry in files table.
    pub fn upsert_file_entry(
        &self,
        path: &str,
        zone: &str,
        file_entry: FileEntryData,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpsertFileEntry {
            path: path.to_string(),
            zone: zone.to_string(),
            file_entry,
        });
    }

    /// Drop file from index by inode.
    pub fn drop_file_index_by_inode(
        &self,
        zone: &str,
        inode: i64,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::DropFileIndexByInode {
            zone: zone.to_string(),
            inode,
        });
    }

    /// Update file path in files table (file moved/renamed).
    /// When new_zone is Some and differs from zone, also migrates zone and tags.
    pub fn update_file_path(
        &self,
        zone: &str,
        inode: i64,
        new_path: &str,
        new_zone: Option<&str>,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpdateFilePath {
            zone: zone.to_string(),
            inode,
            new_path: new_path.to_string(),
            new_zone: new_zone.map(|s| s.to_string()),
        });
    }

    /// Index a directory entry in the files table.
    ///
    /// Used during corpus scanning to track directory entries.
    pub fn index_directory(
        &self,
        path: &str,
        zone: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::IndexDirectory {
            path: path.to_string(),
            zone: zone.to_string(),
            inode,
            mtime_secs,
            mtime_nanos,
        });
    }

    /// Index an image file entry in the files table.
    ///
    /// Used during corpus scanning to register sidecar image files.
    #[allow(clippy::too_many_arguments)] // channel-send boundary; args map 1:1 to DB columns
    pub fn index_image_file(
        &self,
        path: &str,
        zone: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::IndexImageFile {
            path: path.to_string(),
            zone: zone.to_string(),
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
        });
    }

    /// Upsert image metadata into the image_info table.
    pub fn upsert_image_info(
        &self,
        inode: i64,
        format: &str,
        width: u32,
        height: u32,
        role: &str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpsertImageInfo {
            inode,
            format: format.to_string(),
            width,
            height,
            role: role.to_string(),
        });
    }

    /// No-op vestige: tag_mismatches table was dropped. OOB signals handle conflicts now.
    pub fn clear_tag_mismatches_for_track(&self, path: &str, _witness: &MutationExecutionWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearTagMismatchesForTrack {
            path: path.to_string(),
        });
    }

    /// Set the needs_disk_flush flag for a track.
    ///
    /// Used by the DB-first tag editing pattern:
    /// - ApplyTagOps sets this to TRUE after writing tags to DB
    /// - ApplyDbTagsToDisk sets this to FALSE after syncing to disk
    /// - Tracks with TRUE can be recovered via OOB modal
    pub fn set_needs_disk_flush(
        &self,
        path: &str,
        value: bool,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::SetNeedsDiskFlush {
            path: path.to_string(),
            value,
        });
    }

    // =========================================================================
    // Inbox State Operations (Awakening phase cascade cleanup)
    // =========================================================================

    /// Drop all inbox state for an inode no longer observed on disk in inbox.
    ///
    /// Cascade-deletes inbox_tags, files (zone='inbox'), and all per-inode
    /// inbox signals (FileInInbox, InboxUnindexed, InboxHealthy, InboxCorpusMatch)
    /// plus MovedFile. Does NOT touch audio_info or corpus signals.
    pub fn drop_inbox_file_state(&self, inode: i64, _witness: &impl SignalWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::DropInboxFileState { inode });
    }

    // =========================================================================
    // Dirty Inode Operations (for incremental computations)
    // =========================================================================

    /// Clear dirty flag for an inode after successful computation.
    ///
    /// Called by per-inode computations after successfully processing an inode.
    /// This prevents the inode from being reprocessed in the next cycle.
    pub fn clear_dirty_inode(
        &self,
        inode: i64,
        computation_type: &str,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearDirtyInode {
            inode,
            computation_type: computation_type.to_string(),
        });
    }

    /// Mark a batch of inodes dirty for a specific computation type.
    ///
    /// Used when config changes introduce new separators that may affect
    /// existing corpus files. Only processes non-empty batches.
    pub fn mark_dirty_inodes(
        &self,
        inodes: Vec<i64>,
        computation_type: &str,
        _witness: &impl SignalWitness,
    ) {
        if inodes.is_empty() {
            return;
        }
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::MarkDirtyInodes {
            inodes,
            computation_type: computation_type.to_string(),
        });
    }

    // =========================================================================
    // External Matching Operations (fetch thread results — no witness needed)
    // =========================================================================

    /// Insert an external match result from the fetch thread.
    ///
    /// Called by the Witch when draining fetch results, not from computation
    /// or mutation contexts, so no witness is required.
    #[allow(clippy::too_many_arguments)] // channel-send boundary; args map 1:1 to DB columns
    pub fn insert_external_match(
        &self,
        inode: i64,
        fingerprint: Vec<u8>,
        source: i64,
        recording_id: &str,
        confidence: f64,
        raw_response: Option<Vec<u8>>,
        fetched_at: i64,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::InsertExternalMatch {
            inode,
            fingerprint,
            source,
            recording_id: recording_id.to_string(),
            confidence,
            raw_response,
            fetched_at,
        });
    }

    /// Insert a no-match result for a fingerprint.
    pub fn insert_external_no_match(&self, fingerprint: Vec<u8>, source: i64, queried_at: i64) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::InsertExternalNoMatch {
            fingerprint,
            source,
            queried_at,
        });
    }

    /// Upsert an external retry entry (failed lookup).
    pub fn upsert_external_retry(
        &self,
        inode: i64,
        fingerprint: Vec<u8>,
        source: i64,
        error: &str,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpsertExternalRetry {
            inode,
            fingerprint,
            source,
            error: error.to_string(),
        });
    }

    /// Delete an external retry entry (after successful lookup).
    pub fn delete_external_retry(&self, inode: i64, source: i64) {
        self.mark_enqueued();
        let _ = self
            .tx
            .send(DbWriteOp::DeleteExternalRetry { inode, source });
    }

    /// Drop all external match data for an inode.
    pub fn drop_external_match(&self, inode: i64, _witness: &MutationExecutionWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::DropExternalMatch { inode });
    }

    // =========================================================================
    // MusicBrainz Cache Operations (MB fetch thread results — no witness needed)
    // =========================================================================

    /// Upsert a MusicBrainz recording cache entry.
    pub fn upsert_mb_recording_cache(
        &self,
        recording_id: &str,
        raw_json: Vec<u8>,
        fetched_at: i64,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpsertMbCache {
            table: "mb_recording_cache",
            id_col: "recording_id",
            id: recording_id.to_string(),
            raw_json,
            fetched_at,
        });
    }

    /// Upsert a MusicBrainz artist cache entry.
    pub fn upsert_mb_artist_cache(&self, artist_id: &str, raw_json: Vec<u8>, fetched_at: i64) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpsertMbCache {
            table: "mb_artist_cache",
            id_col: "artist_id",
            id: artist_id.to_string(),
            raw_json,
            fetched_at,
        });
    }

    /// Upsert a MusicBrainz release cache entry.
    pub fn upsert_mb_release_cache(&self, release_id: &str, raw_json: Vec<u8>, fetched_at: i64) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::UpsertMbCache {
            table: "mb_release_cache",
            id_col: "release_id",
            id: release_id.to_string(),
            raw_json,
            fetched_at,
        });
    }

    /// Insert a known MusicBrainz entity for resumable fetching.
    pub fn insert_mb_known_entity(
        &self,
        mbid: &str,
        entity_type: &str,
        discovered_from: Option<&str>,
        discovered_at: i64,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::InsertMbKnownEntity {
            mbid: mbid.to_string(),
            entity_type: entity_type.to_string(),
            discovered_from: discovered_from.map(|s| s.to_string()),
            discovered_at,
        });
    }

    // =========================================================================
    // Edit History Purge Operations (operator-confirmed UI action)
    // =========================================================================

    /// Delete all tag edit history rows.
    ///
    /// Operator-confirmed action from the History view, not a corpus mutation,
    /// so no witness is required.
    pub fn clear_tag_edit_history(&self) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearTagEditHistory);
    }

    /// Delete tag edit history rows for a single session.
    pub fn clear_tag_edit_history_session(&self, session_id: &str) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::ClearTagEditHistorySession {
            session_id: session_id.to_string(),
        });
    }

    // =========================================================================
    // Release Packing Pipeline Operations
    // =========================================================================

    /// Truncate both release packing intermediate tables for a fresh pipeline run.
    pub fn truncate_packing_tables(&self, _witness: &impl SignalWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::TruncatePackingTables);
    }

    /// Write a batch of rows to release_packing_manifest.
    pub fn write_packing_manifest(
        &self,
        rows: Vec<(String, i32, String, String, i32)>,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::WritePackingManifest { rows });
    }

    /// Write a batch of scored candidates to release_packing_scores.
    pub fn write_packing_scores(&self, rows: Vec<PackingScoreRow>, _witness: &impl SignalWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::WritePackingScores { rows });
    }

    /// Write a batch of candidate rows to release_packing_candidates.
    pub fn write_packing_candidates(
        &self,
        rows: Vec<PackingCandidateRow>,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::WritePackingCandidates { rows });
    }

    /// Write pending AcoustID submission records (from elimination matching).
    pub fn write_pending_acoustid_submissions(
        &self,
        rows: Vec<PendingAcoustIdSubmission>,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self
            .tx
            .send(DbWriteOp::WritePendingAcoustIdSubmissions { rows });
    }
}
