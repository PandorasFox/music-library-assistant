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

use crate::meta::computations::ComputationWitness;
use crate::meta::signals::{
    AggregateSignalType, CorpusFileSignalType, SignalType,
};
use crate::corpus::db::Database;
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
    pub source: String,     // 'corpus', 'library', 'inbox'
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
    // NOTE: EnsureFileSignal/ClearFileSignal have been removed.
    // - Corpus signals use EnsureCorpusSignal/ClearCorpusSignal (inode-keyed)
    // - Library signals use EnsureAggregateSignal/ClearAggregateSignal (semantic-keyed)

    // =========================================================================
    // Inode-Keyed Corpus Signal Operations
    // =========================================================================

    /// Ensure an inode-keyed corpus signal exists (uses native inode column)
    EnsureCorpusSignal {
        signal_type: CorpusFileSignalType,
        inode: i64,
        path: String,
    },
    /// Clear an inode-keyed corpus signal
    ClearCorpusSignal {
        signal_type: CorpusFileSignalType,
        inode: i64,
    },
    /// Clear all corpus signals for an inode
    ClearAllCorpusSignals {
        inode: i64,
    },

    /// Aggregate signal (with metadata)
    EnsureAggregateSignal {
        signal_type: AggregateSignalType,
        key: String,
        metadata_json: Option<String>,
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
    /// Clear all corpus file signals of a specific type (for bulk re-computation).
    ClearCorpusSignalsByType {
        signal_type: CorpusFileSignalType,
    },

    /// Write a typed signal directly to its per-signal table.
    ///
    /// Bypasses JSON serialization entirely — the typed data struct is sent
    /// through the channel and inserted directly via CorpusSignalStore/AggregateSignalStore.
    WriteTypedSignal {
        signal: crate::meta::signals::data::TypedSignalWrite,
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
    // File/Audio Index Operations (Mutation execution)
    // =========================================================================

    /// Index an audio file (files + audio_info + corpus_tags).
    /// Atomically: insert/replace files row, upsert audio_info, set tags.
    IndexAudioFile {
        path: String,
        file_data: FileData,
        audio_data: AudioData,
        tags: TagSet,
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
    },

    /// Apply incremental tag operations directly.
    /// Used by ApplyTagOps for precise INSERT/DELETE operations.
    ApplyIndexTagOps {
        path: String,
        ops: Vec<crate::meta::mutations::TagOp>,
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
    /// Used by DB-first tag editing pattern: set TRUE after ApplyTagOps,
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

    // NOTE: ensure_file_signal/clear_file_signal have been removed.
    // - Corpus signals: use ensure_corpus_signal/clear_corpus_signal (inode-keyed)
    // - Library signals: use ensure_aggregate_signal/clear_aggregate_signal (semantic-keyed)

    // =========================================================================
    // Inode-keyed corpus signal operations (uses native inode column)
    // =========================================================================

    /// Enqueue an inode-keyed corpus signal (uses native inode column).
    ///
    /// The inode is stored in the dedicated `inode` column for efficient queries.
    /// The path is stored in metadata_json for display purposes.
    pub fn ensure_corpus_signal(
        &self,
        signal_type: CorpusFileSignalType,
        inode: i64,
        path: &str,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::EnsureCorpusSignal {
            signal_type,
            inode,
            path: path.to_string(),
        });
    }

    /// Clear an inode-keyed corpus signal.
    pub fn clear_corpus_signal(
        &self,
        signal_type: CorpusFileSignalType,
        inode: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearCorpusSignal {
            signal_type,
            inode,
        });
    }

    /// Clear all corpus signals for an inode.
    ///
    /// Used when dropping a file from the index to clear all associated signals.
    pub fn clear_all_corpus_signals(&self, inode: i64, _witness: &impl SignalWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearAllCorpusSignals { inode });
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
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::EnsureAggregateSignal {
            signal_type,
            key: key.to_string(),
            metadata_json: metadata_json.map(|s| s.to_string()),
        });
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
        let _ = self.tx.send(SignalWriteOp::WriteTypedSignal { signal });
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

    /// Clear all corpus file signals of a specific type (for bulk re-computation).
    pub fn clear_corpus_signals_by_type(
        &self,
        signal_type: CorpusFileSignalType,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearCorpusSignalsByType { signal_type });
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

    /// Set all tags for a track (replaces existing).
    ///
    /// Used by AssimilateDiskTagsToDb when accepting disk changes.
    /// For incremental tag edits (ApplyTagOps), use `apply_index_tag_ops` instead.
    pub fn set_index_track_tags(
        &self,
        path: &str,
        tags: TagSet,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::SetIndexTrackTags {
            path: path.to_string(),
            tags,
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
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ApplyIndexTagOps {
            path: path.to_string(),
            ops,
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
    /// - ApplyTagOps sets this to TRUE after writing tags to DB
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
        let _ = self.tx.send(SignalWriteOp::ClearDirtyInode {
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

// ============================================================================
// Typed Table Dual-Write Helpers
// ============================================================================
//
// These write to the new per-signal typed tables alongside the old `signals` table.
// During the transition period, both tables are kept in sync. Once reads are
// switched to typed tables (Step B5), the old-table writes will be removed.

use crate::meta::signals::data::*;
use crate::meta::signals::store::{CorpusSignalStore, AggregateSignalStore};

/// Write a simple corpus signal (no extra metadata) to its typed table.
fn typed_write_corpus_signal(db: &Database, signal_type: CorpusFileSignalType, inode: i64, path: &str) {
    let conn = db.conn();
    let _ = match signal_type {
        CorpusFileSignalType::FileInCorpus =>
            FileInCorpusSignal { inode, path: path.to_string() }.insert(conn),
        CorpusFileSignalType::UnindexedFile =>
            UnindexedFileSignal { inode, path: path.to_string() }.insert(conn),
        CorpusFileSignalType::HealthyFile =>
            HealthyFileSignal { inode, path: path.to_string() }.insert(conn),
        CorpusFileSignalType::MissingFile =>
            // No replaced_by_inode available in simple path — use None
            MissingFileSignal { inode, path: path.to_string(), replaced_by_inode: None }.insert(conn),
        CorpusFileSignalType::MissingDirectory =>
            MissingDirectorySignal { inode, path: path.to_string() }.insert(conn),
        CorpusFileSignalType::MtimeOnlyMismatch =>
            MtimeOnlyMismatchSignal { inode, path: path.to_string() }.insert(conn),
        CorpusFileSignalType::CorruptFile =>
            CorruptFileSignal { inode, path: path.to_string() }.insert(conn),
        // These types always have metadata — should go through typed_write_corpus_signal_with_metadata
        CorpusFileSignalType::MovedFile
        | CorpusFileSignalType::ShitFormat
        | CorpusFileSignalType::SubparDuplicate
        | CorpusFileSignalType::CompoundTag
        | CorpusFileSignalType::DeployReady
        | CorpusFileSignalType::DeployedHealthy
        | CorpusFileSignalType::OutOfBandTagSync
        | CorpusFileSignalType::OutOfBandTagConflict => {
            // Called without metadata for a type that normally has it — emit with defaults
            return;
        }
    };
}

/// Write an aggregate signal to its typed table.
fn typed_write_aggregate_signal(
    db: &Database,
    signal_type: AggregateSignalType,
    key: &str,
    metadata_json: Option<&str>,
) {
    let conn = db.conn();
    let meta: serde_json::Value = metadata_json
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    let _ = match signal_type {
        AggregateSignalType::CanonicalTag => {
            CanonicalTagSignal {
                key: key.to_string(),
                tag_name: meta["tag_name"].as_str().unwrap_or("").to_string(),
                canonical_value: meta["canonical_value"].as_str().unwrap_or("").to_string(),
                created_at: meta["created_at"].as_str().unwrap_or("").to_string(),
            }.insert(conn)
        }
        AggregateSignalType::LibraryLeftover => {
            LibraryLeftoverSignal { key: key.to_string() }.insert(conn)
        }
        AggregateSignalType::LibraryStale => {
            LibraryStaleSignal {
                key: key.to_string(),
                library_path: meta["library_path"].as_str().unwrap_or("").to_string(),
                expected_path: meta["expected_path"].as_str().unwrap_or("").to_string(),
                corpus_path: meta["corpus_path"].as_str().unwrap_or("").to_string(),
                inode: meta["inode"].as_i64().unwrap_or(0),
            }.insert(conn)
        }
        AggregateSignalType::FingerprintOverlap => {
            let inodes = parse_inodes_array(&meta);
            FingerprintOverlapSignal { key: key.to_string(), inodes }.insert(conn)
        }
        AggregateSignalType::MetadataDuplicate => {
            let data = MetadataDuplicateData {
                tag_signature: meta["tag_signature"].as_str().unwrap_or("").to_string(),
                inodes: parse_inodes_array(&meta),
            };
            MetadataDuplicateSignal { key: key.to_string(), data }.insert(conn)
        }
        AggregateSignalType::DuplicateInode => {
            let inode_val = meta["inode"].as_i64().unwrap_or(0);
            let inodes = parse_inodes_array(&meta);
            DuplicateInodeSignal { key: key.to_string(), inode: inode_val, inodes }.insert(conn)
        }
        AggregateSignalType::MissingTag => {
            let data = MissingTagData {
                missing_tags: meta["missing_tags"].as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                inodes: parse_inodes_array(&meta),
            };
            MissingTagSignal { key: key.to_string(), data }.insert(conn)
        }
        AggregateSignalType::DeployConflict => {
            let deploy_path = meta["deploy_path"].as_str().unwrap_or("").to_string();
            let inodes = parse_inodes_array(&meta);
            DeployConflictSignal { key: key.to_string(), deploy_path, inodes }.insert(conn)
        }
        AggregateSignalType::TagCanonicity => {
            let data = TagCanonicityData {
                variants: parse_variants_map(&meta),
                inodes: parse_inodes_array(&meta),
            };
            TagCanonicitySignal {
                key: key.to_string(),
                tag_name: meta["tag_name"].as_str().unwrap_or("").to_string(),
                data,
            }.insert(conn)
        }
        AggregateSignalType::InconsistentAlbumArtist => {
            let data = InconsistentAlbumArtistData {
                album: meta["album"].as_str().unwrap_or("").to_string(),
                artist_variants: parse_variants_map_from(&meta, "artist_variants"),
                album_artist_variants: parse_variants_map_from(&meta, "album_artist_variants"),
                inodes: parse_inodes_array(&meta),
            };
            InconsistentAlbumArtistSignal { key: key.to_string(), data }.insert(conn)
        }
        AggregateSignalType::CompoundTagValue => {
            let data = CompoundTagValueData {
                compound_value: meta["compound_value"].as_str().unwrap_or("").to_string(),
                split_parts: meta["split_parts"].as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                separator: meta["separator"].as_str().unwrap_or("").to_string(),
                inodes: parse_inodes_array(&meta),
            };
            CompoundTagValueSignal {
                key: key.to_string(),
                tag_name: meta["tag_name"].as_str().unwrap_or("").to_string(),
                data,
            }.insert(conn)
        }
        AggregateSignalType::CrossSourceOverlap => {
            let data = CrossSourceOverlapData {
                source_a: meta["source_a"].as_str().unwrap_or("").to_string(),
                source_b: meta["source_b"].as_str().unwrap_or("").to_string(),
                source_a_can_stash: meta["source_a_can_stash"].as_bool().unwrap_or(false),
                source_b_can_stash: meta["source_b_can_stash"].as_bool().unwrap_or(false),
                overlap_count: meta["overlap_count"].as_u64().unwrap_or(0) as usize,
                fingerprint_count: meta["fingerprint_count"].as_u64().unwrap_or(0) as usize,
                fingerprint_keys: meta["fingerprint_keys"].as_array()
                    .map(|a| a.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
                    .unwrap_or_default(),
                track_pairs: parse_cross_source_track_pairs(&meta),
            };
            CrossSourceOverlapSignal { key: key.to_string(), data }.insert(conn)
        }
    };
}

/// Clear a corpus signal from its typed table.
fn typed_clear_corpus_signal(db: &Database, signal_type: CorpusFileSignalType, inode: i64) {
    let conn = db.conn();
    let _ = match signal_type {
        CorpusFileSignalType::FileInCorpus => FileInCorpusSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::UnindexedFile => UnindexedFileSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::HealthyFile => HealthyFileSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::MissingFile => MissingFileSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::MissingDirectory => MissingDirectorySignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::MovedFile => MovedFileSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::OutOfBandTagSync => OutOfBandTagSyncSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::OutOfBandTagConflict => OutOfBandTagConflictSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::MtimeOnlyMismatch => MtimeOnlyMismatchSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::CorruptFile => CorruptFileSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::ShitFormat => ShitFormatSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::SubparDuplicate => SubparDuplicateSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::CompoundTag => CompoundTagSignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::DeployReady => DeployReadySignal::clear_by_inode(conn, inode),
        CorpusFileSignalType::DeployedHealthy => DeployedHealthySignal::clear_by_inode(conn, inode),
    };
}

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

/// Clear an aggregate signal from its typed table.
fn typed_clear_aggregate_signal(db: &Database, signal_type: AggregateSignalType, key: &str) {
    let conn = db.conn();
    let _ = match signal_type {
        AggregateSignalType::FingerprintOverlap => FingerprintOverlapSignal::clear_by_key(conn, key),
        AggregateSignalType::MetadataDuplicate => MetadataDuplicateSignal::clear_by_key(conn, key),
        AggregateSignalType::DuplicateInode => DuplicateInodeSignal::clear_by_key(conn, key),
        AggregateSignalType::MissingTag => MissingTagSignal::clear_by_key(conn, key),
        AggregateSignalType::DeployConflict => DeployConflictSignal::clear_by_key(conn, key),
        AggregateSignalType::TagCanonicity => TagCanonicitySignal::clear_by_key(conn, key),
        AggregateSignalType::InconsistentAlbumArtist => InconsistentAlbumArtistSignal::clear_by_key(conn, key),
        AggregateSignalType::CompoundTagValue => CompoundTagValueSignal::clear_by_key(conn, key),
        AggregateSignalType::CrossSourceOverlap => CrossSourceOverlapSignal::clear_by_key(conn, key),
        AggregateSignalType::CanonicalTag => CanonicalTagSignal::clear_by_key(conn, key),
        AggregateSignalType::LibraryLeftover => LibraryLeftoverSignal::clear_by_key(conn, key),
        AggregateSignalType::LibraryStale => LibraryStaleSignal::clear_by_key(conn, key),
    };
}

/// Clear all signals of a given type from typed tables.
fn typed_clear_signals_by_type(db: &Database, issue_type: &SignalType) {
    let conn = db.conn();
    let _ = match issue_type {
        SignalType::FileInCorpus => FileInCorpusSignal::clear_all(conn),
        SignalType::UnindexedFile => UnindexedFileSignal::clear_all(conn),
        SignalType::HealthyFile => HealthyFileSignal::clear_all(conn),
        SignalType::MissingFile => MissingFileSignal::clear_all(conn),
        SignalType::MissingDirectory => MissingDirectorySignal::clear_all(conn),
        SignalType::MovedFile => MovedFileSignal::clear_all(conn),
        SignalType::OutOfBandTagSync => OutOfBandTagSyncSignal::clear_all(conn),
        SignalType::OutOfBandTagConflict => OutOfBandTagConflictSignal::clear_all(conn),
        SignalType::MtimeOnlyMismatch => MtimeOnlyMismatchSignal::clear_all(conn),
        SignalType::CorruptFile => CorruptFileSignal::clear_all(conn),
        SignalType::ShitFormat => ShitFormatSignal::clear_all(conn),
        SignalType::SubparDuplicate => SubparDuplicateSignal::clear_all(conn),
        SignalType::CompoundTagValue => CompoundTagValueSignal::clear_all(conn),
        SignalType::DeployConflict => DeployConflictSignal::clear_all(conn),
        SignalType::FingerprintOverlap => FingerprintOverlapSignal::clear_all(conn),
        SignalType::MetadataDuplicate => MetadataDuplicateSignal::clear_all(conn),
        SignalType::DuplicateInode => DuplicateInodeSignal::clear_all(conn),
        SignalType::MissingTag => MissingTagSignal::clear_all(conn),
        SignalType::TagCanonicity => TagCanonicitySignal::clear_all(conn),
        SignalType::InconsistentAlbumArtist => InconsistentAlbumArtistSignal::clear_all(conn),
        SignalType::CrossSourceOverlap => CrossSourceOverlapSignal::clear_all(conn),
        SignalType::LibraryStale => LibraryStaleSignal::clear_all(conn),
        SignalType::LibraryLeftover => LibraryLeftoverSignal::clear_all(conn),
    };
}

/// Clear all corpus signals of a specific corpus file type from typed tables.
fn typed_clear_corpus_signals_by_type(db: &Database, signal_type: &CorpusFileSignalType) {
    let conn = db.conn();
    let _ = match signal_type {
        CorpusFileSignalType::FileInCorpus => FileInCorpusSignal::clear_all(conn),
        CorpusFileSignalType::UnindexedFile => UnindexedFileSignal::clear_all(conn),
        CorpusFileSignalType::HealthyFile => HealthyFileSignal::clear_all(conn),
        CorpusFileSignalType::MissingFile => MissingFileSignal::clear_all(conn),
        CorpusFileSignalType::MissingDirectory => MissingDirectorySignal::clear_all(conn),
        CorpusFileSignalType::MovedFile => MovedFileSignal::clear_all(conn),
        CorpusFileSignalType::OutOfBandTagSync => OutOfBandTagSyncSignal::clear_all(conn),
        CorpusFileSignalType::OutOfBandTagConflict => OutOfBandTagConflictSignal::clear_all(conn),
        CorpusFileSignalType::MtimeOnlyMismatch => MtimeOnlyMismatchSignal::clear_all(conn),
        CorpusFileSignalType::CorruptFile => CorruptFileSignal::clear_all(conn),
        CorpusFileSignalType::ShitFormat => ShitFormatSignal::clear_all(conn),
        CorpusFileSignalType::SubparDuplicate => SubparDuplicateSignal::clear_all(conn),
        CorpusFileSignalType::CompoundTag => CompoundTagSignal::clear_all(conn),
        CorpusFileSignalType::DeployReady => DeployReadySignal::clear_all(conn),
        CorpusFileSignalType::DeployedHealthy => DeployedHealthySignal::clear_all(conn),
    };
}

// --- JSON parsing helpers for typed table dual-write ---

fn parse_inodes_array(meta: &serde_json::Value) -> Vec<i64> {
    meta["inodes"].as_array()
        .map(|a| a.iter().filter_map(|v| v.as_i64()).collect())
        .unwrap_or_default()
}

fn parse_variants_map(meta: &serde_json::Value) -> Vec<(String, usize)> {
    parse_variants_map_from(meta, "variants")
}

fn parse_variants_map_from(meta: &serde_json::Value, field: &str) -> Vec<(String, usize)> {
    meta[field].as_object()
        .map(|obj| obj.iter().map(|(k, v)| (k.clone(), v.as_u64().unwrap_or(0) as usize)).collect())
        .unwrap_or_default()
}

fn parse_cross_source_track_pairs(meta: &serde_json::Value) -> Vec<CrossSourceTrackPair> {
    meta["track_pairs"].as_array()
        .map(|arr| arr.iter().map(|p| CrossSourceTrackPair {
            fingerprint_key: p["fingerprint_key"].as_str().unwrap_or("").to_string(),
            source_a_inode: p["source_a_inode"].as_i64().unwrap_or(0),
            source_a_path: p["source_a_path"].as_str().unwrap_or("").to_string(),
            source_b_inode: p["source_b_inode"].as_i64().unwrap_or(0),
            source_b_path: p["source_b_path"].as_str().unwrap_or("").to_string(),
        }).collect())
        .unwrap_or_default()
}

/// Execute a single signal write operation.
#[allow(deprecated)]
fn execute_signal_op(db: &Database, op: &SignalWriteOp) {
    // Note: We don't have a ComputationWitness here, but we need one for the db methods.
    // The witness was checked at the send site. We use a thread-local witness for execution.
    let witness = crate::meta::computations::ComputationWitness::new_for_db_thread();

    match op {
        // Inode-keyed corpus signal operations
        SignalWriteOp::EnsureCorpusSignal {
            signal_type,
            inode,
            path,
        } => {
            with_retry("ensure_corpus_signal", path, || {
                db.ensure_corpus_signal(*signal_type, *inode, path, &witness).map(|_| ())
            });
            typed_write_corpus_signal(db, *signal_type, *inode, path);
        }
        SignalWriteOp::ClearCorpusSignal { signal_type, inode } => {
            with_retry("clear_corpus_signal", &inode.to_string(), || {
                db.clear_corpus_signal(*signal_type, *inode, &witness).map(|_| ())
            });
            typed_clear_corpus_signal(db, *signal_type, *inode);
        }
        SignalWriteOp::ClearAllCorpusSignals { inode } => {
            with_retry("clear_all_corpus_signals", &inode.to_string(), || {
                db.clear_all_corpus_signals_for_inode(*inode, &witness).map(|_| ())
            });
            typed_clear_all_corpus_signals(db, *inode);
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
            typed_write_aggregate_signal(db, *signal_type, key, metadata_json.as_deref());
        }
        SignalWriteOp::ClearAggregateSignal { signal_type, key } => {
            with_retry("clear_aggregate_signal", key, || {
                db.clear_aggregate_signal(*signal_type, key, &witness).map(|_| ())
            });
            typed_clear_aggregate_signal(db, *signal_type, key);
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
            typed_clear_signals_by_type(db, issue_type);
        }
        SignalWriteOp::ClearCorpusSignalsByType { signal_type } => {
            let signal_type_str = signal_type.as_str();
            with_retry("clear_corpus_signals_by_type", signal_type_str, || {
                use rusqlite::params;
                db.conn()
                    .execute(
                        "DELETE FROM signals WHERE issue_type = ?1",
                        params![signal_type_str],
                    )
                    .map(|_| ())
                    .map_err(|e: rusqlite::Error| anyhow::anyhow!(e))
            });
            typed_clear_corpus_signals_by_type(db, signal_type);
        }
        SignalWriteOp::WriteTypedSignal { signal } => {
            if let Err(e) = signal.clone().insert(db.conn()) {
                crate::logging::log_error(format!(
                    "[DB_THREAD] write_typed_signal failed: {}", e
                ));
            }
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

        SignalWriteOp::SetIndexTrackTags { path, tags } => {
            with_retry("set_index_track_tags", path, || {
                execute_set_index_track_tags(db, path, tags)
            });
        }

        SignalWriteOp::ApplyIndexTagOps { path, ops } => {
            with_retry("apply_index_tag_ops", path, || {
                execute_apply_index_tag_ops(db, path, ops)
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

        SignalWriteOp::ClearDirtyInode { inode, computation_type } => {
            with_retry("clear_dirty_inode", computation_type, || {
                execute_clear_dirty_inode(db, *inode, computation_type)
            });
        }

        // Shutdown is handled in the run_db_thread loop, never reaches here
        SignalWriteOp::Shutdown => unreachable!("Shutdown handled in run_db_thread loop"),
    }
}

// ============================================================================
// Index Operation Helpers (internal to db_thread)
// ============================================================================

/// Computation types that use per-inode spawning and need dirty tracking.
/// Other computations use bulk SQL queries and don't need this optimization.
const PER_INODE_COMPUTATIONS: &[&str] = &["compound_tag"];

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
) -> anyhow::Result<()> {
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

    let scanned_at = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);

    // Determine which tag table to use based on source
    let tag_table = if file_data.source == "inbox" {
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
    tx.execute(
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

    // Apply tags using the unified helper
    let result = apply_tagset_to_inode(&tx, file_data.inode, tags, tag_table)?;

    // Write history for discovered/changed tags and mark dirty for recomputation
    if result.has_changes() {
        let session_id = format!("index:{}", path);
        let changes = result.to_history_entries();
        write_tag_edit_history(&tx, file_data.inode, &changes, &session_id)?;

        // Mark inode dirty for tag-dependent computations (only for corpus files)
        if file_data.source == "corpus" {
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

    // Clear all inode-keyed signals for this inode (MissingFile, CorruptFile, etc.)
    // This uses the native inode column for proper cleanup
    db.conn().execute(
        "DELETE FROM signals WHERE inode = ?1",
        params![inode],
    )?;

    // Also clear path-keyed signals (MissingDirectory, library signals, etc.)
    // These use issue_key = path
    db.conn().execute(
        "DELETE FROM signals WHERE issue_key = ?1",
        params![path],
    )?;

    Ok(())
}

/// Execute SetIndexTrackTags: replace all tags for a file (corpus_tags).
///
/// Uses apply_tagset_to_inode for atomic diff-based replacement.
/// Writes tag edit history for all changes (this IS an edit, not discovery).
///
/// Used by AssimilateDiskTagsToDb when accepting disk changes.
fn execute_set_index_track_tags(db: &Database, path: &str, tags: &TagSet) -> anyhow::Result<()> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let inode = get_inode_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("File not found: {}", path))?;

    let tx = db.conn().unchecked_transaction()?;

    // Apply tags using the unified helper
    let result = apply_tagset_to_inode(&tx, inode, tags, "corpus_tags")?;

    if result.has_changes() {
        // Increment tags_version using the helper
        increment_tags_version(&tx, inode)?;

        // Write tag edit history using the helper
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let session_id = format!("set_tags:{}", now);
        let changes = result.to_history_entries();
        write_tag_edit_history(&tx, inode, &changes, &session_id)?;

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
) -> anyhow::Result<()> {
    use rusqlite::params;
    use std::time::{SystemTime, UNIX_EPOCH};

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
        let tag_name = op.tag_name.to_lowercase();

        match (&op.old_value, &op.new_value) {
            (Some(old), Some(new)) => {
                // Replace: UPDATE in place
                tx.execute(
                    "UPDATE corpus_tags SET tag_value = ?1 WHERE inode = ?2 AND tag_name = ?3 AND tag_value = ?4",
                    params![new, inode, &tag_name, old],
                )?;
            }
            (Some(old), None) => {
                // Drop: DELETE with exact match
                tx.execute(
                    "DELETE FROM corpus_tags WHERE inode = ?1 AND tag_name = ?2 AND tag_value = ?3",
                    params![inode, &tag_name, old],
                )?;
            }
            (None, Some(new)) => {
                // Add: INSERT OR IGNORE (idempotent - won't fail if already exists)
                tx.execute(
                    "INSERT OR IGNORE INTO corpus_tags (inode, tag_name, tag_value) VALUES (?1, ?2, ?3)",
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

    // Write history entries using the helper
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let session_id = format!("apply_ops:{}", now);
    write_tag_edit_history(&tx, inode, &history_entries, &session_id)?;

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
    let (duration_ms, bitrate_kbps, sample_rate, fingerprint): (Option<i64>, Option<i32>, Option<i32>, Option<Vec<u8>>) =
        tx.query_row(
            "SELECT duration_ms, bitrate_kbps, sample_rate, fingerprint FROM audio_info WHERE inode = ?1",
            params![old_inode],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )?;

    // Insert new audio_info with new file_type
    tx.execute(
        r#"
        INSERT OR REPLACE INTO audio_info
        (inode, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint, needs_tag_flush)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0)
        "#,
        params![new_inode, new_file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint],
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

/// Execute ClearDirtyInode: remove dirty flag after successful computation.
fn execute_clear_dirty_inode(db: &Database, inode: i64, computation_type: &str) -> anyhow::Result<()> {
    use rusqlite::params;

    db.conn().execute(
        "DELETE FROM dirty_inodes WHERE inode = ?1 AND computation_type = ?2",
        params![inode, computation_type],
    )?;

    Ok(())
}

use rusqlite::OptionalExtension;
