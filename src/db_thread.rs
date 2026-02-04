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
//! All write operations are unified into `SignalWriteOp` variants. This includes:
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

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::Instant;

use crate::corpus::computations::ComputationWitness;
use crate::corpus::db::types::{
    AggregateSignal, AggregateSignalType, FileSignalType, SignalType,
};
use crate::corpus::db::Database;
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
    pub source: String,     // 'corpus', 'library', 'inbox'
    pub is_dir: bool,
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
}

/// Track audio/file metadata for indexing (legacy format).
///
/// **DEPRECATED**: Use `FileData` + `AudioData` instead.
/// Retained for backward compatibility during schema migration.
#[derive(Debug, Clone)]
pub struct TrackData {
    pub inode: i64,
    pub file_size: i64,
    pub file_type: String,
    pub duration_ms: Option<i64>,
    pub bitrate_kbps: Option<i32>,
    pub sample_rate: Option<i32>,
    pub fingerprint: Option<Vec<u32>>,
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

/// Signal the DB thread to close its connection and exit.
///
/// Called by `Witch::drop()`. After this, further signal sends will still
/// reach the channel but the thread will have exited, so they queue until
/// process exit.
pub fn request_shutdown() {
    if let Some(sender) = SIGNAL_SENDER.get() {
        let _ = sender.tx.send(SignalWriteOp::Shutdown);
    }
}

// ============================================================================
// Message Types
// ============================================================================

/// Signal write operations (health signals and computation state).
#[derive(Debug)]
enum SignalWriteOp {
    /// File signal (no metadata) - type-safe, preferred
    EnsureFileSignal {
        signal_type: FileSignalType,
        path: String,
    },
    /// File signal with metadata (for signals like LibraryStale that need extra context)
    EnsureFileSignalWithMetadata {
        signal_type: FileSignalType,
        key: String,
        metadata_json: Option<String>,
    },
    /// Clear a file signal
    ClearFileSignal {
        signal_type: FileSignalType,
        path: String,
    },
    /// Aggregate signal (with metadata)
    EnsureAggregateSignal {
        signal_type: AggregateSignalType,
        key: String,
        metadata_json: Option<String>,
    },
    /// Replace an aggregate signal (delete + insert)
    ReplaceAggregateSignal {
        signal: AggregateSignal,
    },
    /// Clear an aggregate signal
    ClearAggregateSignal {
        signal_type: AggregateSignalType,
        key: String,
    },

    // =========================================================================
    // Library File Operations (Awakening phase)
    // =========================================================================

    /// Clear all files for a library before re-scanning.
    ClearLibraryFiles {
        library_name: String,
    },
    /// Record a file discovered during library scanning.
    RecordLibraryFile {
        library_name: String,
        library_root: PathBuf,
        file_path: PathBuf,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
        scanned_at: i64,
    },

    // =========================================================================
    // Bulk Operations (Awake phase content analysis)
    // =========================================================================

    /// Clear all health issues of a specific type (for bulk re-computation).
    ClearSignalsByType {
        issue_type: SignalType,
    },
    /// Update file mtime in files table (after OOB verification).
    /// Uses (source, inode) as the unique key for reliable updates.
    UpdateFileMtime {
        source: String,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
    },

    // =========================================================================
    // Tag Mismatch Operations (OOB verification)
    // =========================================================================

    /// Record a tag mismatch for an audio file (DB differs from disk).
    RecordTagMismatch {
        inode: i64,
        field: String,
        db_value: Option<String>,
        disk_value: Option<String>,
    },
    /// Clear a specific tag mismatch field for an audio file.
    ClearTagMismatch {
        inode: i64,
        field: String,
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
        tags: Vec<(String, String)>,
    },

    /// Drop file from index (file no longer exists or excluded).
    /// Cascades to audio_info, corpus_tags/inbox_tags, tag_edit_history.
    DropFromIndex {
        path: String,
    },

    /// Update track path (file moved/renamed).
    UpdateTrackPath {
        old_path: String,
        new_path: String,
    },

    /// Update track inode (file replaced with same content).
    UpdateTrackInode {
        path: String,
        new_inode: i64,
    },

    /// Set all tags for a track (replaces existing).
    SetTrackTags {
        path: String,
        tags: Vec<(String, String)>,
    },

    /// Update track metadata (full replace for out-of-band changes).
    UpdateTrackMetadata {
        path: String,
        track_data: TrackData,
        tags: Vec<(String, String)>,
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
        source: String,
        file_entry: FileEntryData,
    },

    /// Drop file from index by inode (removes from files table).
    DropFileIndexByInode {
        source: String,
        inode: i64,
    },

    /// Update file path in files table (file moved/renamed).
    UpdateFilePath {
        source: String,
        inode: i64,
        new_path: String,
    },

    /// Cleanup stale file entries (files no longer exist).
    CleanupStaleFiles {
        source: String,
        valid_inodes: Vec<i64>,
    },

    /// Index a directory entry in the files table.
    /// Used during corpus/library scanning to track directory entries.
    IndexDirectory {
        path: String,
        source: String,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
    },

    /// Clear tag mismatches for a track (after resolution).
    ClearTagMismatchesForTrack {
        path: String,
    },

    /// Set the needs_disk_flush flag for a track.
    /// Used by DB-first tag editing pattern: set TRUE after SetTrackTagsDb,
    /// set FALSE after ApplyDbTagsToDisk completes successfully.
    SetNeedsDiskFlush {
        path: String,
        value: bool,
    },

    // TODO: Refactor signal clearing into a unified system with signal categories.
    // File-inherent signals (CorruptFile, ShitFormat) vs tag-based signals (OOB, mtime)
    // should be distinguished at the type level, not via SQL string matching.

    /// Clear all signals for a specific path.
    /// Used by MoveToStash/DropFromIndex to fully clear signals on removal.
    ClearSignalsForPath {
        path: String,
    },

    /// Clear mutable signals for a path, preserving file-inherent signals.
    /// File-inherent signals (CorruptFile, ShitFormat) require specific mutations to clear.
    /// Used by general mutation handler for signal refresh.
    ClearMutableSignalsForPath {
        path: String,
    },

    // =========================================================================
    // Shutdown
    // =========================================================================

    /// Shutdown sentinel — close DB connection and exit thread.
    Shutdown,
}

// IndexWriteOp has been removed - all index operations are now unified into
// SignalWriteOp variants. This simplifies the architecture: one channel,
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
    pub avg_latency_us: u64,
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
            avg_latency_us: if total_writes > 0 {
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
    tx: Sender<SignalWriteOp>,
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
    // Type-safe file signal operations (preferred)
    // =========================================================================

    /// Enqueue a file signal (idempotent create, no metadata).
    pub fn ensure_file_signal(
        &self,
        signal_type: FileSignalType,
        path: &str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::EnsureFileSignal {
            signal_type,
            path: path.to_string(),
        });
    }

    /// Enqueue a file signal with metadata (for signals needing extra context).
    pub fn ensure_file_signal_with_metadata(
        &self,
        signal_type: FileSignalType,
        key: &str,
        metadata_json: Option<&str>,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::EnsureFileSignalWithMetadata {
            signal_type,
            key: key.to_string(),
            metadata_json: metadata_json.map(|s| s.to_string()),
        });
    }

    /// Clear a file signal (idempotent delete).
    pub fn clear_file_signal(
        &self,
        signal_type: FileSignalType,
        path: &str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearFileSignal {
            signal_type,
            path: path.to_string(),
        });
    }

    // =========================================================================
    // Aggregate signal operations
    // =========================================================================

    /// Enqueue an aggregate signal (with metadata).
    pub fn ensure_aggregate_signal(
        &self,
        signal_type: AggregateSignalType,
        key: &str,
        metadata_json: Option<&str>,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::EnsureAggregateSignal {
            signal_type,
            key: key.to_string(),
            metadata_json: metadata_json.map(|s| s.to_string()),
        });
    }

    /// Replace an aggregate signal (delete + insert).
    pub fn replace_aggregate_signal(&self, signal: AggregateSignal, _witness: &ComputationWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ReplaceAggregateSignal { signal });
    }

    /// Clear an aggregate signal (idempotent delete).
    pub fn clear_aggregate_signal(
        &self,
        signal_type: AggregateSignalType,
        key: &str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearAggregateSignal {
            signal_type,
            key: key.to_string(),
        });
    }

    // =========================================================================
    // Library File Operations (Awakening phase)
    // =========================================================================

    /// Clear all files for a library before re-scanning.
    pub fn clear_library_files(&self, library_name: &str, _witness: &ComputationWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearLibraryFiles {
            library_name: library_name.to_string(),
        });
    }

    /// Record a file discovered during library scanning.
    pub fn record_library_file(
        &self,
        library_name: &str,
        library_root: &std::path::Path,
        file_path: &std::path::Path,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
        scanned_at: i64,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::RecordLibraryFile {
            library_name: library_name.to_string(),
            library_root: library_root.to_path_buf(),
            file_path: file_path.to_path_buf(),
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
            scanned_at,
        });
    }

    // =========================================================================
    // Bulk Operations (Awake phase content analysis)
    // =========================================================================

    /// Clear all health issues of a specific type (for bulk re-computation).
    pub fn clear_signals_by_type(
        &self,
        issue_type: SignalType,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearSignalsByType { issue_type });
    }

    /// Update file mtime in files table (after OOB verification).
    /// Uses (source, inode) as the unique key for reliable updates.
    pub fn update_file_mtime(
        &self,
        source: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateFileMtime {
            source: source.to_string(),
            inode,
            mtime_secs,
            mtime_nanos,
        });
    }

    // =========================================================================
    // Tag Mismatch Operations (OOB verification)
    // =========================================================================

    /// Record a tag mismatch for an audio file (routed through db_thread for write access).
    pub fn record_tag_mismatch(
        &self,
        inode: i64,
        field: &str,
        db_value: Option<&str>,
        disk_value: Option<&str>,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::RecordTagMismatch {
            inode,
            field: field.to_string(),
            db_value: db_value.map(|s| s.to_string()),
            disk_value: disk_value.map(|s| s.to_string()),
        });
    }

    /// Clear a specific tag mismatch field for an audio file.
    pub fn clear_tag_mismatch(
        &self,
        inode: i64,
        field: &str,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearTagMismatch {
            inode,
            field: field.to_string(),
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
        tags: Vec<(String, String)>,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::IndexAudioFile {
            path: path.to_string(),
            file_data,
            audio_data,
            tags,
        });
    }

    /// Drop a file from the index by path.
    pub fn drop_from_index(&self, path: &str, _witness: &MutationExecutionWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::DropFromIndex {
            path: path.to_string(),
        });
    }

    /// Update track path (file moved/renamed).
    pub fn update_track_path(
        &self,
        old_path: &str,
        new_path: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateTrackPath {
            old_path: old_path.to_string(),
            new_path: new_path.to_string(),
        });
    }

    /// Update track inode (file replaced).
    pub fn update_track_inode(
        &self,
        path: &str,
        new_inode: i64,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateTrackInode {
            path: path.to_string(),
            new_inode,
        });
    }

    /// Set all tags for a track (replaces existing).
    pub fn set_track_tags(
        &self,
        path: &str,
        tags: Vec<(String, String)>,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::SetTrackTags {
            path: path.to_string(),
            tags,
        });
    }

    /// Update track metadata (full replace for out-of-band changes).
    pub fn update_track_metadata(
        &self,
        path: &str,
        track_data: TrackData,
        tags: Vec<(String, String)>,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateTrackMetadata {
            path: path.to_string(),
            track_data,
            tags,
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
        let _ = self.tx.send(SignalWriteOp::UpdateTrackPathWithMetadata {
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
        source: &str,
        file_entry: FileEntryData,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpsertFileEntry {
            path: path.to_string(),
            source: source.to_string(),
            file_entry,
        });
    }

    /// Drop file from index by inode.
    pub fn drop_file_index_by_inode(
        &self,
        source: &str,
        inode: i64,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::DropFileIndexByInode {
            source: source.to_string(),
            inode,
        });
    }

    /// Update file path in files table (file moved/renamed).
    pub fn update_file_path(
        &self,
        source: &str,
        inode: i64,
        new_path: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateFilePath {
            source: source.to_string(),
            inode,
            new_path: new_path.to_string(),
        });
    }

    /// Cleanup stale file entries (files no longer exist).
    pub fn cleanup_stale_files(
        &self,
        source: &str,
        valid_inodes: Vec<i64>,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::CleanupStaleFiles {
            source: source.to_string(),
            valid_inodes,
        });
    }

    /// Index a directory entry in the files table.
    ///
    /// Used during corpus scanning to track directory entries.
    pub fn index_directory(
        &self,
        path: &str,
        source: &str,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::IndexDirectory {
            path: path.to_string(),
            source: source.to_string(),
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
        let _ = self.tx.send(SignalWriteOp::ClearTagMismatchesForTrack {
            path: path.to_string(),
        });
    }

    /// Clear all signals for a specific path.
    ///
    /// Used by MoveToStash/DropFromIndex to fully clear signals when removing a file.
    pub fn clear_signals_for_path(
        &self,
        path: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearSignalsForPath {
            path: path.to_string(),
        });
    }

    /// Clear mutable signals for a path, preserving file-inherent signals.
    ///
    /// File-inherent signals (CorruptFile, ShitFormat) are preserved because they
    /// require specific mutations or verification to clear. Tag-based signals
    /// are cleared and will be recomputed by UpdateCorpusFileSignals.
    pub fn clear_mutable_signals_for_path(
        &self,
        path: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearMutableSignalsForPath {
            path: path.to_string(),
        });
    }

    /// Set the needs_disk_flush flag for a track.
    ///
    /// Used by the DB-first tag editing pattern:
    /// - SetTrackTagsDb sets this to TRUE after writing tags to DB
    /// - ApplyDbTagsToDisk sets this to FALSE after syncing to disk
    /// - Tracks with TRUE can be recovered via OOB flow
    pub fn set_needs_disk_flush(
        &self,
        path: &str,
        value: bool,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::SetNeedsDiskFlush {
            path: path.to_string(),
            value,
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

    // Create channel (unbounded) - all operations go through SignalWriteOp
    let (signal_tx, signal_rx) = mpsc::channel::<SignalWriteOp>();

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
    signal_rx: Receiver<SignalWriteOp>,
    stats: Arc<SharedStats>,
) {
    // Open database connection (this thread owns the write connection)
    let db = match config::get_db_path().and_then(|p| Database::open(&p).map_err(|e| e.into())) {
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
            Ok(SignalWriteOp::Shutdown) => {
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

/// Execute a single signal write operation.
#[allow(deprecated)]
fn execute_signal_op(db: &Database, op: &SignalWriteOp) {
    // Note: We don't have a ComputationWitness here, but we need one for the db methods.
    // The witness was checked at the send site. We use a thread-local witness for execution.
    let witness = crate::corpus::computations::ComputationWitness::new_for_db_thread();

    match op {
        // Type-safe file signal operations
        SignalWriteOp::EnsureFileSignal { signal_type, path } => {
            with_retry("ensure_file_signal", path, || {
                db.ensure_file_signal(*signal_type, path, &witness).map(|_| ())
            });
        }
        SignalWriteOp::EnsureFileSignalWithMetadata {
            signal_type,
            key,
            metadata_json,
        } => {
            with_retry("ensure_file_signal_with_metadata", key, || {
                db.ensure_file_signal_with_metadata(*signal_type, key, metadata_json.as_deref(), &witness).map(|_| ())
            });
        }
        SignalWriteOp::ClearFileSignal { signal_type, path } => {
            with_retry("clear_file_signal", path, || {
                db.clear_file_signal(*signal_type, path, &witness).map(|_| ())
            });
        }

        // Aggregate signal operations
        SignalWriteOp::EnsureAggregateSignal {
            signal_type,
            key,
            metadata_json,
        } => {
            with_retry("ensure_aggregate_signal", key, || {
                db.ensure_aggregate_signal(*signal_type, key, metadata_json.as_deref(), &witness).map(|_| ())
            });
        }
        SignalWriteOp::ReplaceAggregateSignal { signal } => {
            with_retry("replace_aggregate_signal", &signal.key, || {
                db.replace_aggregate_signal(signal, &witness).map(|_| ())
            });
        }
        SignalWriteOp::ClearAggregateSignal { signal_type, key } => {
            with_retry("clear_aggregate_signal", key, || {
                db.clear_aggregate_signal(*signal_type, key, &witness).map(|_| ())
            });
        }

        // Library file operations (Awakening phase)
        SignalWriteOp::ClearLibraryFiles { library_name } => {
            with_retry("clear_library_files", library_name, || {
                db.clear_library_files(library_name, &witness).map(|_| ())
            });
        }
        SignalWriteOp::RecordLibraryFile {
            library_name,
            library_root,
            file_path,
            inode,
            mtime_secs,
            mtime_nanos,
            file_size,
            scanned_at,
        } => {
            let ctx = file_path.display().to_string();
            with_retry("record_library_file", &ctx, || {
                db.record_library_file(
                    library_name,
                    library_root,
                    file_path,
                    *inode,
                    *mtime_secs,
                    *mtime_nanos,
                    *file_size,
                    *scanned_at,
                    &witness,
                )
            });
        }

        // Bulk operations (Awake phase content analysis)
        SignalWriteOp::ClearSignalsByType { issue_type } => {
            let issue_type_str = issue_type.as_str();
            with_retry("clear_signals_by_type", issue_type_str, || {
                use rusqlite::params;
                db.conn()
                    .execute(
                        "DELETE FROM signals WHERE issue_type = ?1",
                        params![issue_type_str],
                    )
                    .map(|_| ())
                    .map_err(|e: rusqlite::Error| anyhow::anyhow!(e))
            });
        }
        SignalWriteOp::UpdateFileMtime {
            source,
            inode,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("update_file_mtime", source, || {
                use rusqlite::params;
                let rows_affected = db.conn()
                    .execute(
                        "UPDATE files SET mtime_secs = ?1, mtime_nanos = ?2 WHERE source = ?3 AND inode = ?4",
                        params![mtime_secs, mtime_nanos, source, inode],
                    )
                    .map_err(|e: rusqlite::Error| anyhow::anyhow!(e))?;
                if rows_affected == 0 {
                    crate::logging::log_error(format!(
                        "[DB_THREAD] update_file_mtime: no rows matched for source={}, inode={}",
                        source, inode
                    ));
                }
                Ok(())
            });
        }

        // Tag mismatch operations (OOB verification)
        SignalWriteOp::RecordTagMismatch {
            inode,
            field,
            db_value,
            disk_value,
        } => {
            with_retry("record_tag_mismatch", field, || {
                db.record_tag_mismatch(*inode, field, db_value.as_deref(), disk_value.as_deref(), &witness)
            });
        }
        SignalWriteOp::ClearTagMismatch { inode, field } => {
            with_retry("clear_tag_mismatch", field, || {
                db.clear_tag_mismatch(*inode, field, &witness)
            });
        }

        // =====================================================================
        // File/Audio Index Operations (Mutation execution)
        // =====================================================================

        SignalWriteOp::IndexAudioFile {
            path,
            file_data,
            audio_data,
            tags,
        } => {
            with_retry("index_audio_file", path, || {
                execute_index_audio_file(db, path, file_data, audio_data, tags)
            });
        }

        SignalWriteOp::DropFromIndex { path } => {
            with_retry("drop_from_index", path, || {
                execute_drop_from_index(db, path)
            });
        }

        SignalWriteOp::UpdateTrackPath { old_path, new_path } => {
            with_retry("update_track_path", old_path, || {
                execute_update_track_path(db, old_path, new_path)
            });
        }

        SignalWriteOp::UpdateTrackInode { path, new_inode } => {
            with_retry("update_track_inode", path, || {
                execute_update_track_inode(db, path, *new_inode)
            });
        }

        SignalWriteOp::SetTrackTags { path, tags } => {
            with_retry("set_track_tags", path, || {
                execute_set_track_tags(db, path, tags)
            });
        }

        SignalWriteOp::UpdateTrackMetadata { path, track_data, tags } => {
            with_retry("update_track_metadata", path, || {
                execute_update_track_metadata(db, path, track_data, tags)
            });
        }

        SignalWriteOp::UpdateTrackPathWithMetadata {
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

        SignalWriteOp::UpsertFileEntry { path, source, file_entry } => {
            with_retry("upsert_file_entry", path, || {
                execute_upsert_file_entry(db, path, source, file_entry)
            });
        }

        SignalWriteOp::DropFileIndexByInode { source, inode } => {
            with_retry("drop_file_index_by_inode", source, || {
                db.drop_file_index_by_inode(source, *inode, &witness).map(|_| ())
            });
        }

        SignalWriteOp::UpdateFilePath { source, inode, new_path } => {
            with_retry("update_file_path", new_path, || {
                db.update_file_path(source, *inode, new_path, &witness)
            });
        }

        SignalWriteOp::CleanupStaleFiles { source, valid_inodes } => {
            with_retry("cleanup_stale_files", source, || {
                let valid_set: std::collections::HashSet<i64> = valid_inodes.iter().copied().collect();
                db.cleanup_stale_files(source, &valid_set, &witness).map(|_| ())
            });
        }

        SignalWriteOp::IndexDirectory {
            path,
            source,
            inode,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("index_directory", path, || {
                execute_index_directory(db, path, source, *inode, *mtime_secs, *mtime_nanos)
            });
        }

        SignalWriteOp::ClearTagMismatchesForTrack { path } => {
            with_retry("clear_tag_mismatches_for_track", path, || {
                execute_clear_tag_mismatches_for_track(db, path)
            });
        }

        SignalWriteOp::ClearSignalsForPath { path } => {
            with_retry("clear_signals_for_path", path, || {
                db.delete_signals_for_path(path, &witness).map(|_| ())
            });
        }

        SignalWriteOp::ClearMutableSignalsForPath { path } => {
            with_retry("clear_mutable_signals_for_path", path, || {
                db.delete_mutable_signals_for_path(path, &witness).map(|_| ())
            });
        }

        SignalWriteOp::SetNeedsDiskFlush { path, value } => {
            with_retry("set_needs_disk_flush", path, || {
                execute_set_needs_disk_flush(db, path, *value)
            });
        }

        // Shutdown is handled in the run_db_thread loop, never reaches here
        SignalWriteOp::Shutdown => unreachable!("Shutdown handled in run_db_thread loop"),
    }
}

// ============================================================================
// Index Operation Helpers (internal to db_thread)
// ============================================================================

/// Convert fingerprint Vec<u32> to BLOB bytes (little-endian).
fn fingerprint_to_blob(fp: &[u32]) -> Vec<u8> {
    fp.iter().flat_map(|n| n.to_le_bytes()).collect()
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
fn execute_index_audio_file(
    db: &Database,
    path: &str,
    file_data: &FileData,
    audio_data: &AudioData,
    tags: &[(String, String)],
) -> anyhow::Result<()> {
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Insert or replace files row
    db.conn().execute(
        r#"
        INSERT OR REPLACE INTO files
        (inode, source, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
        "#,
        params![
            file_data.inode,
            &file_data.source,
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
    db.conn().execute(
        r#"
        INSERT OR REPLACE INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
        "#,
        params![
            file_data.inode,
            &audio_data.file_type,
            audio_data.duration_ms,
            audio_data.bitrate_kbps,
            audio_data.sample_rate,
            &fp_blob,
        ],
    )?;

    // Determine which tag table to use based on source
    let tag_table = if file_data.source == "inbox" {
        "inbox_tags"
    } else {
        "corpus_tags"
    };

    // Delete existing tags for this inode
    db.conn().execute(
        &format!("DELETE FROM {} WHERE inode = ?1", tag_table),
        params![file_data.inode],
    )?;

    // Insert new tags
    let insert_sql = format!(
        "INSERT INTO {} (inode, tag_name, tag_value) VALUES (?1, ?2, ?3)",
        tag_table
    );
    for (name, value) in tags {
        if !value.is_empty() {
            db.conn().execute(&insert_sql, params![file_data.inode, name, value])?;
        }
    }

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

    // Clear all signals for this path (MissingFile, CorruptFile, etc.)
    // This is the resolution action - dropping from index resolves file-level signals
    db.conn().execute(
        "DELETE FROM signals WHERE issue_key = ?1",
        params![path],
    )?;

    Ok(())
}

/// Execute UpdateTrackPath: update path for relocated file.
fn execute_update_track_path(db: &Database, old_path: &str, new_path: &str) -> anyhow::Result<()> {
    use rusqlite::params;

    // Update files table
    db.conn().execute(
        "UPDATE files SET path = ?1 WHERE path = ?2",
        params![new_path, old_path],
    )?;

    Ok(())
}

/// Execute UpdateTrackInode: update inode for replaced file.
/// This handles the case where a file's content is replaced (new inode).
fn execute_update_track_inode(db: &Database, path: &str, new_inode: i64) -> anyhow::Result<()> {
    use rusqlite::params;

    // Get old inode first
    let old_inode = match get_inode_by_path(db, path)? {
        Some(id) => id,
        None => return Ok(()), // File doesn't exist
    };

    // Update files table inode
    db.conn().execute(
        "UPDATE files SET inode = ?1 WHERE path = ?2",
        params![new_inode, path],
    )?;

    // Move audio_info to new inode if old inode has no other references
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM files WHERE inode = ?1",
        params![old_inode],
        |row| row.get(0),
    )?;
    if count == 0 {
        // Copy audio_info to new inode
        db.conn().execute(
            "INSERT OR REPLACE INTO audio_info SELECT ?1, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, needs_tag_flush FROM audio_info WHERE inode = ?2",
            params![new_inode, old_inode],
        )?;
        // Copy tags to new inode
        db.conn().execute(
            "INSERT OR IGNORE INTO corpus_tags SELECT ?1, tag_name, tag_value FROM corpus_tags WHERE inode = ?2",
            params![new_inode, old_inode],
        )?;
        // Delete old entries
        db.conn().execute("DELETE FROM audio_info WHERE inode = ?1", params![old_inode])?;
        db.conn().execute("DELETE FROM corpus_tags WHERE inode = ?1", params![old_inode])?;
    }

    Ok(())
}

/// Execute SetTrackTags: replace all tags for a file (corpus_tags).
fn execute_set_track_tags(db: &Database, path: &str, tags: &[(String, String)]) -> anyhow::Result<()> {
    use rusqlite::params;

    let inode = get_inode_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

    // Delete existing tags
    db.conn().execute(
        "DELETE FROM corpus_tags WHERE inode = ?1",
        params![inode],
    )?;

    // Insert new tags
    for (name, value) in tags {
        if !value.is_empty() {
            db.conn().execute(
                "INSERT INTO corpus_tags (inode, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                params![inode, name, value],
            )?;
        }
    }

    Ok(())
}

/// Execute UpdateTrackMetadata: full replace for out-of-band changes.
/// Updates files, audio_info, and corpus_tags.
fn execute_update_track_metadata(
    db: &Database,
    path: &str,
    track_data: &TrackData,
    tags: &[(String, String)],
) -> anyhow::Result<()> {
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let inode = get_inode_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Update files table (inode, file_size)
    db.conn().execute(
        "UPDATE files SET inode = ?1, file_size = ?2, scanned_at = ?3 WHERE path = ?4",
        params![track_data.inode, track_data.file_size, scanned_at, path],
    )?;

    // Convert fingerprint to BLOB if present
    let fp_blob: Option<Vec<u8>> = track_data.fingerprint.as_ref().map(|fp| fingerprint_to_blob(fp));

    // Update audio_info (or insert if new inode)
    db.conn().execute(
        r#"
        INSERT OR REPLACE INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
        "#,
        params![
            track_data.inode,
            &track_data.file_type,
            track_data.duration_ms,
            track_data.bitrate_kbps,
            track_data.sample_rate,
            &fp_blob,
        ],
    )?;

    // Replace tags in corpus_tags
    db.conn().execute(
        "DELETE FROM corpus_tags WHERE inode = ?1",
        params![inode],
    )?;

    for (name, value) in tags {
        if !value.is_empty() {
            db.conn().execute(
                "INSERT INTO corpus_tags (inode, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                params![track_data.inode, name, value],
            )?;
        }
    }

    // Clean up old inode's audio_info if orphaned
    if inode != track_data.inode {
        let count: i64 = db.conn().query_row(
            "SELECT COUNT(*) FROM files WHERE inode = ?1",
            params![inode],
            |row| row.get(0),
        )?;
        if count == 0 {
            db.conn().execute("DELETE FROM audio_info WHERE inode = ?1", params![inode])?;
            db.conn().execute("DELETE FROM corpus_tags WHERE inode = ?1", params![inode])?;
        }
    }

    Ok(())
}

/// Execute UpdateTrackPathWithMetadata: update path and file metadata for transcoded file.
/// Updates files table path/inode/file_size, and audio_info file_type.
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

    // Update files table
    let rows_updated = db.conn().execute(
        "UPDATE files SET path = ?1, inode = ?2, file_size = ?3, scanned_at = ?4 WHERE path = ?5",
        params![new_path, new_inode, new_file_size, scanned_at, old_path],
    )?;

    if rows_updated == 0 {
        anyhow::bail!("File not found at old path: {}", old_path);
    }

    // Get old audio_info to copy to new inode
    let (duration_ms, bitrate_kbps, sample_rate, fingerprint): (Option<i64>, Option<i32>, Option<i32>, Option<Vec<u8>>) =
        db.conn().query_row(
            "SELECT duration_ms, bitrate_kbps, sample_rate, fingerprint FROM audio_info WHERE inode = ?1",
            params![old_inode],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

    // Insert new audio_info with new file_type
    db.conn().execute(
        r#"
        INSERT OR REPLACE INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
        "#,
        params![new_inode, new_file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint],
    )?;

    // Copy tags to new inode
    db.conn().execute(
        "INSERT OR IGNORE INTO corpus_tags SELECT ?1, tag_name, tag_value FROM corpus_tags WHERE inode = ?2",
        params![new_inode, old_inode],
    )?;

    // Clean up old inode if orphaned
    if old_inode != new_inode {
        let count: i64 = db.conn().query_row(
            "SELECT COUNT(*) FROM files WHERE inode = ?1",
            params![old_inode],
            |row| row.get(0),
        )?;
        if count == 0 {
            db.conn().execute("DELETE FROM audio_info WHERE inode = ?1", params![old_inode])?;
            db.conn().execute("DELETE FROM corpus_tags WHERE inode = ?1", params![old_inode])?;
        }
    }

    Ok(())
}

/// Execute UpsertFileEntry: insert or update file entry in files table.
fn execute_upsert_file_entry(
    db: &Database,
    path: &str,
    source: &str,
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
        INSERT INTO files (inode, source, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, 0, ?4, ?5, ?6, ?7)
        ON CONFLICT(inode, source, path) DO UPDATE SET
            mtime_secs = excluded.mtime_secs,
            mtime_nanos = excluded.mtime_nanos,
            file_size = excluded.file_size,
            scanned_at = excluded.scanned_at
        "#,
        params![
            file_entry.inode,
            source,
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
/// now handled via OOB signals with mismatch details in metadata_json.
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
    source: &str,
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
        (inode, source, path, is_dir, mtime_secs, mtime_nanos, file_size, scanned_at)
        VALUES (?1, ?2, ?3, 1, ?4, ?5, 0, ?6)
        "#,
        params![
            inode,
            source,
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

use rusqlite::OptionalExtension;
