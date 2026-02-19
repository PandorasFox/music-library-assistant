//! Dedicated DB write thread for eliminating connection contention.
//!
//! Provides fire-and-forget write operations via typed channels, preserving
//! witness semantics at call sites. The thread owns a single write connection
//! and processes operations sequentially, eliminating lock contention.
//!
//! ## Architecture
//!
//! - `SignalWriteSender`: All write operations (signals, index updates, tag edits)
//! - `DbThreadHandle`: Stats access and shutdown coordination
//!
//! All write operations are unified into `DbWriteOp` variants. This includes:
//! - Health signals (corpus file signals, aggregate signals, library signals)
//! - Index operations (track inserts, updates, deletes)
//! - Tag operations (edits, sets, history logging)
//! - File entry operations (upsert, mtime updates, cleanup)
//!
//! ## Witness Semantics
//!
//! Witnesses are required at the *send* call site, not at execution time.
//! This preserves the invariant that only legitimate execution contexts can
//! enqueue writes, even though the actual DB operation happens asynchronously.
//! Both `ComputationWitness` and `MutationExecutionWitness` implement `SignalWitness`.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::meta::computations::ComputationWitness;
use crate::db::Database;
use crate::corpus::tags::TagSet;
use crate::witch::MutationExecutionWitness;
use crate::config;

// ============================================================================
// Index Signal Data Types
// ============================================================================

/// File entry metadata for the files table.
///
/// Used when indexing a file or directory into the files table.
#[derive(Debug, Clone)]
pub struct FileData {
    pub inode: i64,
    pub zone: String,       // 'corpus', 'library', 'inbox'
    pub _is_dir: bool,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}

/// Audio-specific metadata for the audio_info table.
///
/// Only for audio files (not directories).
#[derive(Debug, Clone)]
pub struct AudioData {
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub fingerprint: Option<Vec<u32>>,
    pub has_pictures: bool,
}

/// File entry data for files table operations.
///
/// Used for upsert operations that don't need full FileData (e.g., UpdateFileEntry).
/// Contains the core file identity and mtime fields stored in the files table.
#[derive(Debug, Clone)]
pub struct FileEntryData {
    pub inode: i64,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}

// ============================================================================
// Signal Witness Trait
// ============================================================================

/// Marker trait for types that authorize signal emission.
///
/// Both `ComputationWitness` (computation context) and `MutationExecutionWitness`
/// (mutation context) implement this trait. Signal emission methods accept
/// `&impl SignalWitness` to work in either context.
///
/// Creation restrictions on each witness type ensure signals can only be
/// emitted from authorized execution contexts.
pub trait SignalWitness {}

impl SignalWitness for ComputationWitness {}
impl SignalWitness for MutationExecutionWitness {}

// ============================================================================
// Global Sender Access
// ============================================================================

/// Global signal sender, initialized when db thread is spawned.
static SIGNAL_SENDER: OnceLock<SignalWriteSender> = OnceLock::new();

/// Get the global signal sender for computation execution.
///
/// Returns None if the db thread hasn't been spawned yet.
/// Computation code should use this to obtain the sender.
pub fn signal_sender() -> Option<&'static SignalWriteSender> {
    SIGNAL_SENDER.get()
}

/// Block until all queued DB operations have been processed.
///
/// Computations call this when they need to ensure their writes are visible
/// to subsequent computations that read those signals. The computation
/// blocks itself; the sender continues to accept writes.
///
/// Returns false if db thread not initialized.
pub fn wait_for_queue_drain() -> bool {
    use rand::Rng;

    if let Some(sender) = SIGNAL_SENDER.get() {
        // Spin with small sleeps to avoid busy-waiting
        let mut rng = rand::thread_rng();
        while !sender.stats.queue_empty.load(Ordering::Acquire) {
            // Roll a d20 and sleep for that many millis
            let d20: u64 = rng.gen_range(1..=20);
            std::thread::sleep(std::time::Duration::from_millis(d20));
        }
        true
    } else {
        false
    }
}

/// Execute VACUUM on the db_thread's write connection.
///
/// Blocks the caller until VACUUM completes. Called during startup before
/// any read-only connections exist, so the write connection has exclusive access.
pub fn execute_vacuum() -> Result<(), String> {
    let sender = SIGNAL_SENDER.get()
        .ok_or_else(|| "db_thread not initialized".to_string())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    sender.mark_enqueued();
    let _ = sender.tx.send(DbWriteOp::ExecuteVacuum { result_tx: tx });
    rx.recv().map_err(|_| "db_thread disconnected during VACUUM".to_string())?
}

/// Apply a schema migration on the db_thread's write connection.
///
/// Blocks the caller until the migration completes. The migration runs on the
/// db_thread which owns the write connection, eliminating the need for a
/// separate write connection opened from the rayon thread pool.
pub fn execute_migration(migration_id: u32) -> Result<(), String> {
    let sender = SIGNAL_SENDER.get()
        .ok_or_else(|| "db_thread not initialized".to_string())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    sender.mark_enqueued();
    let _ = sender.tx.send(DbWriteOp::ApplyMigration { migration_id, result_tx: tx });
    rx.recv().map_err(|_| "db_thread disconnected during migration".to_string())?
}

/// Signal the DB thread to close its connection and exit.
///
/// Called by `Witch::drop()`. After this, further signal sends will still
/// reach the channel but the thread will have exited, so they queue until
/// process exit.
pub fn request_shutdown() {
    if let Some(sender) = SIGNAL_SENDER.get() {
        let _ = sender.tx.send(DbWriteOp::Shutdown);
    }
}

// ============================================================================
// Message Types
// ============================================================================

/// Signal write operations (health signals and computation state).
#[derive(Debug)]
enum DbWriteOp {
    // =========================================================================
    // Signal Clear Operations (function-pointer dispatch)
    // =========================================================================

    /// Clear a single corpus signal by inode (function pointer resolved at send time).
    ClearCorpusSignalByInode {
        clear_fn: fn(&rusqlite::Connection, i64) -> rusqlite::Result<()>,
        inode: i64,
        label: &'static str,
    },
    /// Clear all corpus signals for an inode (hardcoded list).
    ClearAllCorpusSignals {
        inode: i64,
    },
    /// Clear mutable corpus signals for an inode (preserves CorruptFile, ShitFormat).
    ClearMutableCorpusSignals {
        inode: i64,
    },
    /// Clear a single aggregate signal by key (function pointer resolved at send time).
    ClearAggregateSignalByKey {
        clear_fn: fn(&rusqlite::Connection, &str) -> rusqlite::Result<()>,
        key: String,
        label: &'static str,
    },
    /// Clear all aggregate signals whose key starts with the given prefix.
    ClearAggregateByKeyPrefix {
        clear_fn: fn(&rusqlite::Connection, &str) -> rusqlite::Result<()>,
        prefix: String,
        label: &'static str,
    },

    // =========================================================================
    // Library File Operations (Awakening phase - reconciliation)
    // =========================================================================

    /// Upsert a library file during reconciliation (new or changed).
    UpsertLibraryFile {
        stored_path: String,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
    },
    /// Delete a stale library file during reconciliation.
    DeleteLibraryFile {
        stored_path: String,
    },

    // =========================================================================
    // Bulk Operations (Awake phase content analysis)
    // =========================================================================

    /// Write a typed signal directly to its per-signal table.
    ///
    /// Bypasses JSON serialization entirely — the typed data struct is sent
    /// through the channel and inserted directly via CorpusSignalStore/AggregateSignalStore.
    WriteTypedSignal {
        signal: crate::meta::signals::data::TypedSignalWrite,
    },
    /// Update file mtime in files table (after OOB verification).
    /// Uses (zone, inode) as the unique key for reliable updates.
    UpdateFileMtime {
        zone: String,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
    },

    // =========================================================================
    // File/Audio Index Operations (Mutation execution)
    // =========================================================================

    /// Index an audio file (files + audio_info + corpus_tags).
    /// Atomically: insert/replace files row, upsert audio_info, set tags.
    IndexAudioFile {
        path: String,
        file_data: FileData,
        audio_data: AudioData,
        tags: TagSet,
        session_id: String,
    },

    /// Drop file from index (file no longer exists or excluded).
    /// Cascades to audio_info, corpus_tags/inbox_tags, tag_edit_history.
    DropFromIndex {
        path: String,
    },

    /// Set all tags for a track (replaces existing).
    /// Used by AssimilateDiskTagsToDb when accepting disk changes.
    SetIndexTrackTags {
        path: String,
        tags: TagSet,
        tag_table: String,
        session_id: String,
    },

    /// Apply incremental tag operations directly.
    /// Used by ApplyTagOps for precise INSERT/DELETE operations.
    ApplyIndexTagOps {
        path: String,
        ops: Vec<crate::meta::mutations::TagOp>,
        tag_table: String,
        session_id: String,
    },

    /// Update track path and file metadata (for transcode/format conversion).
    /// Looks up track by old_path, updates to new_path with new file metadata.
    UpdateTrackPathWithMetadata {
        old_path: String,
        new_path: String,
        new_inode: i64,
        new_file_size: i64,
        new_file_type: String,
    },

    // =========================================================================
    // File Entry Operations (for mutations)
    // =========================================================================

    /// Upsert file entry in files table.
    UpsertFileEntry {
        path: String,
        zone: String,
        file_entry: FileEntryData,
    },

    /// Drop file from index by inode (removes from files table).
    DropFileIndexByInode {
        zone: String,
        inode: i64,
    },

    /// Update file path in files table (file moved/renamed).
    /// When new_zone differs from zone, also updates zone and migrates tags.
    UpdateFilePath {
        zone: String,
        inode: i64,
        new_path: String,
        new_zone: Option<String>,
    },

    /// Index a directory entry in the files table.
    /// Used during corpus/library scanning to track directory entries.
    IndexDirectory {
        path: String,
        zone: String,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
    },

    /// Clear tag mismatches for a track (after resolution).
    ClearTagMismatchesForTrack {
        path: String,
    },

    /// Set the needs_disk_flush flag for a track.
    /// Used by DB-first tag editing pattern: set TRUE after ApplyTagOps,
    /// set FALSE after ApplyDbTagsToDisk completes successfully.
    SetNeedsDiskFlush {
        path: String,
        value: bool,
    },

    /// Mark a file as having embedded pictures and update its mtime/size.
    /// Sent by EmbedAlbumArt after successful embed (or no-op when art already exists).
    /// Combines has_pictures update (audio_info) with mtime+size update (files)
    /// to prevent both signal re-emission and OOB mtime mismatch detection.
    SetHasPictures {
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
    },

    // TODO: Refactor signal clearing into a unified system with signal categories.
    // File-inherent signals (CorruptFile, ShitFormat) vs tag-based signals (OOB, mtime)
    // should be distinguished at the type level, not via SQL string matching.

    // =========================================================================
    // Inbox State Operations (Awakening phase cascade cleanup)
    // =========================================================================

    /// Drop all inbox state for an inode no longer observed on disk.
    ///
    /// Cascade-deletes inbox_tags, files (zone='inbox'), and all per-inode
    /// inbox signals. Does NOT touch audio_info (corpus may share the inode
    /// after a move) or corpus signals.
    DropInboxFileState {
        inode: i64,
    },

    // =========================================================================
    // Dirty Inode Operations (for incremental computations)
    // =========================================================================

    /// Clear dirty flag for an inode after successful computation.
    ClearDirtyInode {
        inode: i64,
        computation_type: String,
    },

    // =========================================================================
    // Shutdown
    // =========================================================================

    /// Execute VACUUM on the write connection. Handled in main loop (like Shutdown).
    ExecuteVacuum {
        result_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
    },

    /// Apply a schema migration on the write connection. Handled in main loop (like Shutdown).
    ApplyMigration {
        migration_id: u32,
        result_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
    },

    /// Shutdown sentinel — close DB connection and exit thread.
    Shutdown,
}

// IndexWriteOp has been removed - all index operations are now unified into
// DbWriteOp variants. This simplifies the architecture: one channel,
// one sender, one execution loop.

// ============================================================================
// Shared Stats (Atomic Counters)
// ============================================================================

/// Shared statistics between DB thread and handle.
///
/// All fields are atomic for lock-free access from UI thread.
/// Timing-related fields are only updated when `timing_enabled` is true.
struct SharedStats {
    /// Whether timing instrumentation is enabled
    timing_enabled: bool,
    /// Total write operations processed (timing only)
    total_writes: AtomicU64,
    /// Signal operations (timing only)
    signal_writes: AtomicU64,
    /// Index operations (Phase 2, timing only)
    index_writes: AtomicU64,
    /// Current queue depth - always tracked for shutdown coordination
    queue_depth: AtomicU64,
    /// Cumulative microseconds spent in DB operations (timing only)
    total_db_time_us: AtomicU64,
    /// True when queue is empty - always tracked for shutdown blocking
    queue_empty: AtomicBool,
    /// Thread start time for rate calculations (timing only)
    start_time: Instant,
}

impl SharedStats {
    fn new(timing_enabled: bool) -> Self {
        Self {
            timing_enabled,
            total_writes: AtomicU64::new(0),
            signal_writes: AtomicU64::new(0),
            index_writes: AtomicU64::new(0),
            queue_depth: AtomicU64::new(0),
            total_db_time_us: AtomicU64::new(0),
            queue_empty: AtomicBool::new(true),
            start_time: Instant::now(),
        }
    }
}

// ============================================================================
// Public Types
// ============================================================================

/// Statistics snapshot for UI display.
#[derive(Debug, Clone, Default)]
pub struct DbThreadStats {
    pub total_writes: u64,
    pub _signal_writes: u64,
    pub _index_writes: u64,
    pub queue_depth: u64,
    pub _avg_latency_us: u64,
    pub writes_per_sec: f64,
    pub _queue_empty: bool,
}

/// Handle to the DB thread for stats access and shutdown coordination.
pub struct DbThreadHandle {
    stats: Arc<SharedStats>,
    thread_handle: Option<JoinHandle<()>>,
}

impl DbThreadHandle {
    /// Join the DB thread, blocking until it finishes closing connections.
    pub fn join(&mut self) {
        if let Some(handle) = self.thread_handle.take() {
            let _ = handle.join();
        }
    }

    /// Check if the write queue is empty (for shutdown blocking).
    pub fn queue_empty(&self) -> bool {
        self.stats.queue_empty.load(Ordering::Acquire)
    }

    /// Get the current queue depth (always available, regardless of timing).
    pub fn queue_depth(&self) -> u64 {
        self.stats.queue_depth.load(Ordering::Relaxed)
    }

    /// Get current stats snapshot for UI display.
    /// Returns None if timing instrumentation is disabled.
    pub fn stats(&self) -> Option<DbThreadStats> {
        if !self.stats.timing_enabled {
            return None;
        }

        let total_writes = self.stats.total_writes.load(Ordering::Relaxed);
        let total_db_time_us = self.stats.total_db_time_us.load(Ordering::Relaxed);
        let elapsed_secs = self.stats.start_time.elapsed().as_secs_f64();

        Some(DbThreadStats {
            total_writes,
            _signal_writes: self.stats.signal_writes.load(Ordering::Relaxed),
            _index_writes: self.stats.index_writes.load(Ordering::Relaxed),
            queue_depth: self.stats.queue_depth.load(Ordering::Relaxed),
            _avg_latency_us: if total_writes > 0 {
                total_db_time_us / total_writes
            } else {
                0
            },
            writes_per_sec: if elapsed_secs > 0.0 {
                total_writes as f64 / elapsed_secs
            } else {
                0.0
            },
            _queue_empty: self.stats.queue_empty.load(Ordering::Acquire),
        })
    }
}

/// Sender for signal write operations.
///
/// Clone-able, thread-safe. All methods require `ComputationWitness` to ensure
/// only computation execution contexts can enqueue signal writes.
#[derive(Clone)]
pub struct SignalWriteSender {
    tx: Sender<DbWriteOp>,
    stats: Arc<SharedStats>,
}

impl SignalWriteSender {
    /// Update queue stats when enqueuing an operation.
    #[inline]
    fn mark_enqueued(&self) {
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

    /// Write a typed signal directly to its per-signal table.
    ///
    /// This is the typed-data path that bypasses JSON serialization.
    /// Computations construct the typed data struct and send it directly.
    pub fn write_typed_signal(
        &self,
        signal: crate::meta::signals::data::TypedSignalWrite,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::WriteTypedSignal { signal });
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
    pub fn delete_library_file(
        &self,
        stored_path: &str,
        _witness: &ComputationWitness,
    ) {
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
        tags: TagSet,
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
        tags: TagSet,
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

    /// Clear all tag mismatches for a track.
    pub fn clear_tag_mismatches_for_track(
        &self,
        path: &str,
        _witness: &MutationExecutionWitness,
    ) {
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

    /// Mark a file as having embedded pictures and update its mtime/size.
    ///
    /// Called by EmbedAlbumArt after successful embed (or no-op when art
    /// already exists). Updates audio_info.has_pictures = 1 and syncs the
    /// file's mtime+size in the files table to prevent OOB mismatch detection.
    pub fn set_has_pictures(
        &self,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(DbWriteOp::SetHasPictures {
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
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
    pub fn drop_inbox_file_state(
        &self,
        inode: i64,
        _witness: &impl SignalWitness,
    ) {
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
}

// IndexWriteSender has been removed - all operations now go through SignalWriteSender.

// ============================================================================
// DB Thread Implementation
// ============================================================================

/// Spawn the DB write thread.
///
/// Returns the handle for stats/shutdown. Also initializes the global signal sender
/// so computation code can access it via `signal_sender()`.
pub fn spawn() -> DbThreadHandle {
    let timing_enabled = config::is_timing_enabled();
    let stats = Arc::new(SharedStats::new(timing_enabled));

    // Create channel (unbounded) - all operations go through DbWriteOp
    let (signal_tx, signal_rx) = mpsc::channel::<DbWriteOp>();

    let thread_stats = Arc::clone(&stats);
    let thread_handle = thread::spawn(move || {
        run_db_thread(signal_rx, thread_stats);
    });

    let handle = DbThreadHandle {
        stats: Arc::clone(&stats),
        thread_handle: Some(thread_handle),
    };

    let signal_sender = SignalWriteSender {
        tx: signal_tx,
        stats: Arc::clone(&stats),
    };

    // Initialize global sender (computation and mutation code accesses via signal_sender())
    let _ = SIGNAL_SENDER.set(signal_sender);

    handle
}

/// Main loop for the DB write thread.
fn run_db_thread(
    signal_rx: Receiver<DbWriteOp>,
    stats: Arc<SharedStats>,
) {
    // Open database connection (this thread owns the write connection)
    let db = match config::get_db_path().and_then(|p| Database::open(&p)) {
        Ok(db) => db,
        Err(e) => {
            crate::logging::log_error(format!("[DB_THREAD] Failed to open database: {}", e));
            return;
        }
    };

    crate::logging::log_general("[DB_THREAD] Started");

    // Process signal operations
    // Note: Using recv() which blocks until a message arrives or channel closes
    let timing_enabled = stats.timing_enabled;

    loop {
        match signal_rx.recv() {
            Ok(DbWriteOp::ExecuteVacuum { result_tx }) => {
                crate::logging::log_general("[DB_THREAD] Executing VACUUM");
                let result = db.conn().execute_batch("VACUUM")
                    .map_err(|e| format!("{}", e));
                if result.is_ok() {
                    crate::logging::log_general("[DB_THREAD] VACUUM completed");
                }
                let _ = result_tx.send(result);
                stats.queue_depth.fetch_sub(1, Ordering::Relaxed);
                if stats.queue_depth.load(Ordering::Relaxed) == 0 {
                    stats.queue_empty.store(true, Ordering::Release);
                }
            }
            Ok(DbWriteOp::ApplyMigration { migration_id, result_tx }) => {
                crate::logging::log_general(format!(
                    "[DB_THREAD] Applying migration v{}", migration_id
                ));
                let witness = crate::witch::MaintenanceWitness::new_for_db_thread();
                let registry = crate::meta::mutations::MigrationRegistry::new();
                let result = registry.apply_migration(&db, migration_id, &witness)
                    .map_err(|e| format!("{:#}", e));
                if result.is_ok() {
                    crate::logging::log_general(format!(
                        "[DB_THREAD] Migration v{} completed", migration_id
                    ));
                }
                let _ = result_tx.send(result);
                stats.queue_depth.fetch_sub(1, Ordering::Relaxed);
                if stats.queue_depth.load(Ordering::Relaxed) == 0 {
                    stats.queue_empty.store(true, Ordering::Release);
                }
            }
            Ok(DbWriteOp::Shutdown) => {
                crate::logging::log_general(
                    "[DB_THREAD] Shutdown requested, closing database connection",
                );
                break;
            }
            Ok(op) => {
                // Only time operations when instrumentation is enabled
                if timing_enabled {
                    let start = Instant::now();
                    execute_signal_op(&db, &op);
                    let elapsed_us = start.elapsed().as_micros() as u64;

                    // Update timing stats
                    stats.total_writes.fetch_add(1, Ordering::Relaxed);
                    stats.signal_writes.fetch_add(1, Ordering::Relaxed);
                    stats.total_db_time_us.fetch_add(elapsed_us, Ordering::Relaxed);
                } else {
                    execute_signal_op(&db, &op);
                }

                // Always update queue management (needed for shutdown coordination)
                stats.queue_depth.fetch_sub(1, Ordering::Relaxed);
                if stats.queue_depth.load(Ordering::Relaxed) == 0 {
                    stats.queue_empty.store(true, Ordering::Release);
                }
            }
            Err(_) => {
                // Channel closed, thread should exit
                crate::logging::log_general("[DB_THREAD] Channel closed, exiting");
                break;
            }
        }
    }

    // Checkpoint WAL before closing to ensure -wal and -shm files are cleaned up.
    // TRUNCATE mode checkpoints fully and truncates WAL to zero length.
    // This must happen after all read connections have closed.
    match db.conn().execute_batch("PRAGMA wal_checkpoint(TRUNCATE);") {
        Ok(()) => {
            crate::logging::log_general("[DB_THREAD] WAL checkpoint completed");
        }
        Err(e) => {
            crate::logging::log_error(format!("[DB_THREAD] WAL checkpoint failed: {}", e));
        }
    }

    // Explicitly close the database connection
    drop(db);
    crate::logging::log_general("[DB_THREAD] Database connection closed");
}

/// Retry constants for transient SQLite errors.
const MAX_RETRIES: u32 = 3;
const BASE_DELAY_MS: u64 = 50;

/// Check if an error is a retryable SQLite error and return the reason if so.
fn retryable_sqlite_error(e: &anyhow::Error) -> Option<String> {
    e.chain().find_map(|cause| {
        if let Some(sqlite_err) = cause.downcast_ref::<rusqlite::Error>() {
            match sqlite_err {
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error { code: rusqlite::ffi::ErrorCode::DatabaseBusy, extended_code },
                    msg
                ) => Some(format!("SQLITE_BUSY (ext={}): {:?}", extended_code, msg)),
                rusqlite::Error::SqliteFailure(
                    rusqlite::ffi::Error { code: rusqlite::ffi::ErrorCode::DatabaseLocked, extended_code },
                    msg
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
                            op_name, attempt + 1, MAX_RETRIES, delay, reason, context
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
//
// These operate on the per-signal typed tables, which are now the sole source
// of truth for signal data.

use crate::meta::signals::data::*;
use crate::meta::signals::store::CorpusSignalStore;

/// Clear ALL corpus signals for an inode from typed tables.
fn typed_clear_all_corpus_signals(db: &Database, inode: i64) {
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
    let _ = CorruptFileSignal::clear_by_inode(conn, inode);
    let _ = ShitFormatSignal::clear_by_inode(conn, inode);
    let _ = SubparDuplicateSignal::clear_by_inode(conn, inode);
    let _ = CompoundTagSignal::clear_by_inode(conn, inode);
    let _ = DeployReadySignal::clear_by_inode(conn, inode);
    let _ = DeployedHealthySignal::clear_by_inode(conn, inode);
}

/// Clear mutable corpus signals for an inode (preserves CorruptFile, ShitFormat).
///
/// File-inherent signals (CorruptFile, ShitFormat) represent intrinsic file
/// properties discovered during indexing. They should persist across mutations
/// that don't remove/replace the file.
fn typed_clear_mutable_corpus_signals(db: &Database, inode: i64) {
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
    // CorruptFile and ShitFormat intentionally preserved
    let _ = SubparDuplicateSignal::clear_by_inode(conn, inode);
    let _ = CompoundTagSignal::clear_by_inode(conn, inode);
    let _ = DeployReadySignal::clear_by_inode(conn, inode);
    let _ = DeployedHealthySignal::clear_by_inode(conn, inode);
}

/// Execute a single signal write operation.
fn execute_signal_op(db: &Database, op: &DbWriteOp) {
    // Note: We don't have a ComputationWitness here, but we need one for the db methods.
    // The witness was checked at the send site. We use a thread-local witness for execution.
    let witness = crate::meta::computations::ComputationWitness::new_for_db_thread();

    match op {
        // Signal clear operations (function-pointer dispatch)
        DbWriteOp::ClearCorpusSignalByInode { clear_fn, inode, label } => {
            if let Err(e) = clear_fn(db.conn(), *inode) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] clear {} by inode {} failed: {}", label, inode, e
                ));
            }
        }
        DbWriteOp::ClearAllCorpusSignals { inode } => {
            typed_clear_all_corpus_signals(db, *inode);
        }
        DbWriteOp::ClearMutableCorpusSignals { inode } => {
            typed_clear_mutable_corpus_signals(db, *inode);
        }
        DbWriteOp::ClearAggregateSignalByKey { clear_fn, key, label } => {
            if let Err(e) = clear_fn(db.conn(), key) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] clear {} by key '{}' failed: {}", label, key, e
                ));
            }
        }
        DbWriteOp::ClearAggregateByKeyPrefix { clear_fn, prefix, label } => {
            if let Err(e) = clear_fn(db.conn(), prefix) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] clear {} by prefix '{}' failed: {}", label, prefix, e
                ));
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
                execute_upsert_library_file(db, stored_path, *inode, *mtime_secs, *mtime_nanos, *file_size)
            });
        }
        DbWriteOp::DeleteLibraryFile { stored_path } => {
            with_retry("delete_library_file", stored_path, || {
                execute_delete_library_file(db, stored_path)
            });
        }

        DbWriteOp::WriteTypedSignal { signal } => {
            if let Err(e) = signal.clone().insert(db.conn()) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] write_typed_signal failed: {}", e
                ));
            }
        }
        DbWriteOp::UpdateFileMtime {
            zone,
            inode,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("update_file_mtime", zone, || {
                use rusqlite::params;
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

        DbWriteOp::SetIndexTrackTags { path, tags, tag_table, session_id } => {
            with_retry("set_index_track_tags", path, || {
                execute_set_index_track_tags(db, path, tags, tag_table, session_id)
            });
        }

        DbWriteOp::ApplyIndexTagOps { path, ops, tag_table, session_id } => {
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
                    db, old_path, new_path, *new_inode, *new_file_size, new_file_type,
                )
            });
        }

        DbWriteOp::UpsertFileEntry { path, zone, file_entry } => {
            with_retry("upsert_file_entry", path, || {
                execute_upsert_file_entry(db, path, zone, file_entry)
            });
        }

        DbWriteOp::DropFileIndexByInode { zone, inode } => {
            with_retry("drop_file_index_by_inode", zone, || {
                db.drop_file_index_by_inode(zone, *inode, &witness).map(|_| ())
            });
        }

        DbWriteOp::UpdateFilePath { zone, inode, new_path, new_zone } => {
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

        DbWriteOp::SetHasPictures { inode, mtime_secs, mtime_nanos, file_size } => {
            with_retry("set_has_pictures", &inode.to_string(), || {
                execute_set_has_pictures(db, *inode, *mtime_secs, *mtime_nanos, *file_size)
            });
        }

        DbWriteOp::DropInboxFileState { inode } => {
            with_retry("drop_inbox_file_state", &inode.to_string(), || {
                execute_drop_inbox_file_state(db, *inode)
            });
        }

        DbWriteOp::ClearDirtyInode { inode, computation_type } => {
            with_retry("clear_dirty_inode", computation_type, || {
                execute_clear_dirty_inode(db, *inode, computation_type)
            });
        }

        // ExecuteVacuum, ApplyMigration, and Shutdown are handled in the run_db_thread loop, never reach here
        DbWriteOp::ExecuteVacuum { .. } => unreachable!("ExecuteVacuum handled in run_db_thread loop"),
        DbWriteOp::ApplyMigration { .. } => unreachable!("ApplyMigration handled in run_db_thread loop"),
        DbWriteOp::Shutdown => unreachable!("Shutdown handled in run_db_thread loop"),
    }
}

// ============================================================================
// Index Operation Helpers (internal to db_thread)
// ============================================================================

/// Computation types that use per-inode spawning and need dirty tracking.
/// Other computations use bulk SQL queries and don't need this optimization.
const PER_INODE_COMPUTATIONS: &[&str] = &["compound_tag", "shit_format"];

/// Mark an inode as dirty for all per-inode computations.
/// Called whenever tags change for a corpus file.
fn mark_inode_dirty(conn: &rusqlite::Connection, inode: i64) -> anyhow::Result<()> {
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    for computation_type in PER_INODE_COMPUTATIONS {
        conn.execute(
            "INSERT OR REPLACE INTO dirty_inodes (inode, computation_type, dirtied_at) VALUES (?1, ?2, ?3)",
            params![inode, *computation_type, now],
        )?;
    }

    Ok(())
}

/// Increment the tags_version counter for an inode.
/// Called whenever tags are modified (not on initial indexing).
fn increment_tags_version(conn: &rusqlite::Connection, inode: i64) -> anyhow::Result<()> {
    use rusqlite::params;
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
    use rusqlite::params;

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
    use rusqlite::params;

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
    let removed: Vec<(String, String)> = diff.only_left.iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    let added: Vec<(String, String)> = diff.only_right.iter()
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
    use rusqlite::params;
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
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

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
    let fp_blob: Option<Vec<u8>> = audio_data.fingerprint.as_ref().map(|fp| fingerprint_to_blob(fp));

    // Insert or replace audio_info row
    tx.execute(
        r#"
        INSERT OR REPLACE INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
        "#,
        params![
            file_data.inode,
            &audio_data.file_type,
            audio_data.duration_ms,
            audio_data.bitrate_kbps,
            audio_data.sample_rate,
            &fp_blob,
            audio_data.has_pictures as i32,
        ],
    )?;

    // Apply tags using the unified helper
    let result = apply_tagset_to_inode(&tx, file_data.inode, tags, tag_table)?;

    // Write history for discovered/changed tags and mark dirty for recomputation
    if result.has_changes() {
        let changes = result.to_history_entries();
        write_tag_edit_history(&tx, file_data.inode, &changes, session_id)?;

        // Mark inode dirty for tag-dependent computations (only for corpus files)
        if file_data.zone == "corpus" {
            mark_inode_dirty(&tx, file_data.inode)?;
        }
    }

    tx.commit()?;
    Ok(())
}

/// Execute DropFromIndex: delete file entry and cascades.
/// audio_info and tags cascade via foreign keys.
fn execute_drop_from_index(db: &Database, path: &str) -> anyhow::Result<()> {
    use rusqlite::params;

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
    db.conn().execute(
        "DELETE FROM files WHERE path = ?1",
        params![path],
    )?;

    // If no other paths reference this inode, clean up audio_info
    // (FK CASCADE should handle this, but be explicit)
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM files WHERE inode = ?1",
        params![inode],
        |row| row.get(0),
    )?;
    if count == 0 {
        db.conn().execute(
            "DELETE FROM audio_info WHERE inode = ?1",
            params![inode],
        )?;
    }

    // Clear all corpus signals for this inode from typed tables
    typed_clear_all_corpus_signals(db, inode);

    Ok(())
}

/// Execute SetIndexTrackTags: replace all tags for a file (corpus_tags).
///
/// Uses apply_tagset_to_inode for atomic diff-based replacement.
/// Writes tag edit history for all changes (this IS an edit, not discovery).
///
/// Used by AssimilateDiskTagsToDb when accepting disk changes.
fn execute_set_index_track_tags(db: &Database, path: &str, tags: &TagSet, tag_table: &str, session_id: &str) -> anyhow::Result<()> {

    let inode = get_inode_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

    let tx = db.conn().unchecked_transaction()?;

    // Apply tags using the unified helper
    let result = apply_tagset_to_inode(&tx, inode, tags, tag_table)?;

    if result.has_changes() {
        // Increment tags_version using the helper
        increment_tags_version(&tx, inode)?;

        // Write tag edit history using the caller-provided session identifier
        let changes = result.to_history_entries();
        write_tag_edit_history(&tx, inode, &changes, session_id)?;

        // Mark inode dirty for tag-dependent computations
        mark_inode_dirty(&tx, inode)?;
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
    use rusqlite::params;

    let inode = get_inode_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

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
                    &format!("INSERT OR IGNORE INTO {} (inode, tag_name, tag_value) VALUES (?1, ?2, ?3)", tag_table),
                    params![inode, &tag_name, new],
                )?;
            }
            (None, None) => {
                // No-op - should not reach here due to filter
            }
        }

        // Collect history entry for this operation
        history_entries.push((
            tag_name,
            op.old_value.clone(),
            op.new_value.clone(),
        ));
    }

    // Write history entries using the caller-provided session identifier
    write_tag_edit_history(&tx, inode, &history_entries, session_id)?;

    // Increment tags_version using the helper
    increment_tags_version(&tx, inode)?;

    // Mark inode dirty for tag-dependent computations
    mark_inode_dirty(&tx, inode)?;

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
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    // Get old inode
    let old_inode = get_inode_by_path(db, old_path)?
        .ok_or_else(|| anyhow::anyhow!("File not found at old path: {}", old_path))?;

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

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
    let (duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures): (Option<i64>, Option<i32>, Option<i32>, Option<Vec<u8>>, i32) =
        tx.query_row(
            "SELECT duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures FROM audio_info WHERE inode = ?1",
            params![old_inode],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get::<_, i32>(4).unwrap_or(0))),
        )?;

    // Insert new audio_info with new file_type
    tx.execute(
        r#"
        INSERT OR REPLACE INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, has_pictures, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 0)
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
            tx.execute("DELETE FROM audio_info WHERE inode = ?1", params![old_inode])?;
            tx.execute("DELETE FROM corpus_tags WHERE inode = ?1", params![old_inode])?;
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
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

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
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Insert or replace directory entry
    db.conn().execute(
        r#"
        INSERT OR REPLACE INTO files
        (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, 1, ?4, ?5, 0, ?6)
        "#,
        params![
            inode,
            zone,
            path,
            mtime_secs,
            mtime_nanos,
            scanned_at,
        ],
    )?;

    Ok(())
}

/// Execute SetNeedsDiskFlush: update the needs_tag_flush flag on audio_info.
fn execute_set_needs_disk_flush(db: &Database, path: &str, value: bool) -> anyhow::Result<()> {
    use rusqlite::params;

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

/// Execute SetHasPictures: mark audio_info.has_pictures = 1 and update file mtime/size.
fn execute_set_has_pictures(
    db: &Database,
    inode: i64,
    mtime_secs: i64,
    mtime_nanos: i64,
    file_size: i64,
) -> anyhow::Result<()> {
    use rusqlite::params;

    let rows = db.conn().execute(
        "UPDATE audio_info SET has_pictures = 1 WHERE inode = ?1",
        params![inode],
    )?;
    if rows == 0 {
        crate::logging::log_error(format!(
            "[DB_THREAD] set_has_pictures: no audio_info for inode={}", inode
        ));
    }

    db.conn().execute(
        "UPDATE files SET mtime_secs = ?1, mtime_nanos = ?2, file_size = ?3 \
         WHERE inode = ?4 AND zone = 'corpus'",
        params![mtime_secs, mtime_nanos, file_size, inode],
    )?;

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
    use rusqlite::params;

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
fn execute_clear_dirty_inode(db: &Database, inode: i64, computation_type: &str) -> anyhow::Result<()> {
    use rusqlite::params;

    db.conn().execute(
        "DELETE FROM dirty_inodes WHERE inode = ?1 AND computation_type = ?2",
        params![inode, computation_type],
    )?;

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
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    db.conn().execute(
        "INSERT OR REPLACE INTO files
         (inode, zone, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
         VALUES (?1, 'library', ?2, 0, ?3, ?4, ?5, ?6)",
        params![inode, stored_path, mtime_secs, mtime_nanos, file_size, scanned_at],
    )?;

    Ok(())
}

/// Execute DeleteLibraryFile: remove a stale library file from the files table.
fn execute_delete_library_file(
    db: &Database,
    stored_path: &str,
) -> anyhow::Result<()> {
    use rusqlite::params;

    db.conn().execute(
        "DELETE FROM files WHERE zone = 'library' AND path = ?1",
        params![stored_path],
    )?;

    Ok(())
}

use rusqlite::OptionalExtension;
