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
//! - Scan state operations (upsert, mtime updates, cleanup)
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

/// Track audio/file metadata for indexing (no path, no id - those are signal keys).
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

/// Scan state metadata for incremental scanning.
#[derive(Debug, Clone)]
pub struct ScanStateData {
    pub inode: i64,
    pub mtime_secs: i64,
    pub mtime_nanos: i64,
    pub file_size: i64,
}

/// Single tag edit operation for surgical edits.
#[derive(Debug, Clone)]
pub struct TagEditOp {
    pub tag_name: String,
    pub old_value: Option<String>,
    pub new_value: Option<String>,
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
    /// Clear signals in a directory (for file signals)
    ClearFileSignalsInDirectory {
        directory: PathBuf,
        signal_type: FileSignalType,
    },

    // =========================================================================
    // Library Scan State Operations (Awakening phase)
    // =========================================================================

    /// Clear all scan state for a library before re-scanning.
    ClearLibraryScanState {
        library_name: String,
    },
    /// Record a file discovered during library scanning.
    RecordLibraryFile {
        library_name: String,
        library_root: PathBuf,
        file_path: PathBuf,
        inode: i64,
        scanned_at: i64,
    },

    // =========================================================================
    // Bulk Operations (Awake phase content analysis)
    // =========================================================================

    /// Clear all health issues of a specific type (for bulk re-computation).
    ClearSignalsByType {
        issue_type: SignalType,
    },
    /// Update scan_state mtime for a file (after OOB verification).
    UpdateScanStateMtime {
        path: String,
        mtime_secs: i64,
        mtime_nanos: i64,
    },

    // =========================================================================
    // Tag Mismatch Operations (OOB verification)
    // =========================================================================

    /// Record a tag mismatch for a track (DB differs from disk).
    RecordTagMismatch {
        track_id: i64,
        field: String,
        db_value: Option<String>,
        disk_value: Option<String>,
    },
    /// Clear a specific tag mismatch field for a track.
    ClearTagMismatch {
        track_id: i64,
        field: String,
    },

    // =========================================================================
    // Track Index Operations (Mutation execution)
    // =========================================================================

    /// Index a track (observed file state → DB).
    /// Atomically: insert/replace track, set tags, upsert scan_state.
    IndexTrack {
        path: String,
        source: String,
        track_data: TrackData,
        tags: Vec<(String, String)>,
        scan_state: ScanStateData,
    },

    /// Drop track from index (file no longer exists or excluded).
    /// Cascades to tags, scan_state, tag_edit_history.
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

    /// Surgical tag edits with history logging.
    EditTrackTags {
        path: String,
        edits: Vec<TagEditOp>,
        session_id: String,
    },

    /// Log a tag edit to history (for audit trail).
    LogTagEdit {
        path: String,
        tag_name: String,
        old_value: Option<String>,
        new_value: Option<String>,
        session_id: String,
    },

    /// Update a single tag value.
    UpdateTrackTag {
        path: String,
        tag_name: String,
        new_value: String,
    },

    /// Delete a specific tag from a track.
    DeleteTrackTag {
        path: String,
        tag_name: String,
    },

    /// Update track metadata (full replace for out-of-band changes).
    UpdateTrackMetadata {
        path: String,
        track_data: TrackData,
        tags: Vec<(String, String)>,
    },

    // =========================================================================
    // Scan State Operations (for mutations)
    // =========================================================================

    /// Upsert scan state entry.
    UpsertScanState {
        path: String,
        source: String,
        scan_state: ScanStateData,
    },

    /// Delete scan state by inode.
    DeleteScanStateByInode {
        source: String,
        inode: i64,
    },

    /// Update scan state path (file moved/renamed).
    UpdateScanStatePath {
        source: String,
        inode: i64,
        new_path: String,
    },

    /// Cleanup stale scan state entries (files no longer exist).
    CleanupStaleScanState {
        source: String,
        valid_inodes: Vec<i64>,
    },

    /// Clear tag mismatches for a track (after resolution).
    ClearTagMismatchesForTrack {
        path: String,
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
    pub signal_writes: u64,
    pub index_writes: u64,
    pub queue_depth: u64,
    pub avg_latency_us: u64,
    pub writes_per_sec: f64,
    pub queue_empty: bool,
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

    /// Check if timing instrumentation is enabled.
    pub fn timing_enabled(&self) -> bool {
        self.stats.timing_enabled
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
            signal_writes: self.stats.signal_writes.load(Ordering::Relaxed),
            index_writes: self.stats.index_writes.load(Ordering::Relaxed),
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
            queue_empty: self.stats.queue_empty.load(Ordering::Acquire),
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

    /// Clear all file signals of a type in a directory.
    pub fn clear_file_signals_in_directory(
        &self,
        directory: &std::path::Path,
        signal_type: FileSignalType,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearFileSignalsInDirectory {
            directory: directory.to_path_buf(),
            signal_type,
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
    // Library Scan State Operations (Awakening phase)
    // =========================================================================

    /// Clear all scan state for a library before re-scanning.
    pub fn clear_library_scan_state(&self, library_name: &str, _witness: &ComputationWitness) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearLibraryScanState {
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
        scanned_at: i64,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::RecordLibraryFile {
            library_name: library_name.to_string(),
            library_root: library_root.to_path_buf(),
            file_path: file_path.to_path_buf(),
            inode,
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

    /// Update scan_state mtime for a file (after OOB verification).
    pub fn update_scan_state_mtime(
        &self,
        path: &str,
        mtime_secs: i64,
        mtime_nanos: i64,
        _witness: &impl SignalWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateScanStateMtime {
            path: path.to_string(),
            mtime_secs,
            mtime_nanos,
        });
    }

    // =========================================================================
    // Tag Mismatch Operations (OOB verification)
    // =========================================================================

    /// Record a tag mismatch for a track (routed through db_thread for write access).
    pub fn record_tag_mismatch(
        &self,
        track_id: i64,
        field: &str,
        db_value: Option<&str>,
        disk_value: Option<&str>,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::RecordTagMismatch {
            track_id,
            field: field.to_string(),
            db_value: db_value.map(|s| s.to_string()),
            disk_value: disk_value.map(|s| s.to_string()),
        });
    }

    /// Clear a specific tag mismatch field for a track.
    pub fn clear_tag_mismatch(
        &self,
        track_id: i64,
        field: &str,
        _witness: &ComputationWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::ClearTagMismatch {
            track_id,
            field: field.to_string(),
        });
    }

    // =========================================================================
    // Track Index Operations (Mutation execution)
    // =========================================================================

    /// Index a track (atomically insert/replace track, set tags, upsert scan_state).
    pub fn index_track(
        &self,
        path: &str,
        source: &str,
        track_data: TrackData,
        tags: Vec<(String, String)>,
        scan_state: ScanStateData,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::IndexTrack {
            path: path.to_string(),
            source: source.to_string(),
            track_data,
            tags,
            scan_state,
        });
    }

    /// Drop a track from the index by path.
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

    /// Apply surgical tag edits with history logging.
    pub fn edit_track_tags(
        &self,
        path: &str,
        edits: Vec<TagEditOp>,
        session_id: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::EditTrackTags {
            path: path.to_string(),
            edits,
            session_id: session_id.to_string(),
        });
    }

    /// Log a tag edit to history.
    pub fn log_tag_edit(
        &self,
        path: &str,
        tag_name: &str,
        old_value: Option<&str>,
        new_value: Option<&str>,
        session_id: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::LogTagEdit {
            path: path.to_string(),
            tag_name: tag_name.to_string(),
            old_value: old_value.map(|s| s.to_string()),
            new_value: new_value.map(|s| s.to_string()),
            session_id: session_id.to_string(),
        });
    }

    /// Update a single tag value.
    pub fn update_track_tag(
        &self,
        path: &str,
        tag_name: &str,
        new_value: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateTrackTag {
            path: path.to_string(),
            tag_name: tag_name.to_string(),
            new_value: new_value.to_string(),
        });
    }

    /// Delete a specific tag from a track.
    pub fn delete_track_tag(
        &self,
        path: &str,
        tag_name: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::DeleteTrackTag {
            path: path.to_string(),
            tag_name: tag_name.to_string(),
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

    /// Upsert scan state entry.
    pub fn upsert_scan_state(
        &self,
        path: &str,
        source: &str,
        scan_state: ScanStateData,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpsertScanState {
            path: path.to_string(),
            source: source.to_string(),
            scan_state,
        });
    }

    /// Delete scan state by inode.
    pub fn delete_scan_state_by_inode(
        &self,
        source: &str,
        inode: i64,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::DeleteScanStateByInode {
            source: source.to_string(),
            inode,
        });
    }

    /// Update scan state path (file moved/renamed).
    pub fn update_scan_state_path(
        &self,
        source: &str,
        inode: i64,
        new_path: &str,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::UpdateScanStatePath {
            source: source.to_string(),
            inode,
            new_path: new_path.to_string(),
        });
    }

    /// Cleanup stale scan state entries (files no longer exist).
    pub fn cleanup_stale_scan_state(
        &self,
        source: &str,
        valid_inodes: Vec<i64>,
        _witness: &MutationExecutionWitness,
    ) {
        self.mark_enqueued();
        let _ = self.tx.send(SignalWriteOp::CleanupStaleScanState {
            source: source.to_string(),
            valid_inodes,
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
        SignalWriteOp::ClearFileSignalsInDirectory {
            directory,
            signal_type,
        } => {
            let ctx = directory.display().to_string();
            with_retry("clear_file_signals_in_directory", &ctx, || {
                db.clear_file_signals_in_directory(directory, *signal_type, &witness).map(|_| ())
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

        // Library scan state operations (Awakening phase)
        SignalWriteOp::ClearLibraryScanState { library_name } => {
            with_retry("clear_library_scan_state", library_name, || {
                db.clear_library_scan_state(library_name, &witness).map(|_| ())
            });
        }
        SignalWriteOp::RecordLibraryFile {
            library_name,
            library_root,
            file_path,
            inode,
            scanned_at,
        } => {
            let ctx = file_path.display().to_string();
            with_retry("record_library_file", &ctx, || {
                db.record_library_file(library_name, library_root, file_path, *inode, *scanned_at, &witness)
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
        SignalWriteOp::UpdateScanStateMtime {
            path,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("update_scan_state_mtime", path, || {
                use rusqlite::params;
                db.conn()
                    .execute(
                        "UPDATE scan_state SET mtime_secs = ?1, mtime_nanos = ?2 WHERE path = ?3",
                        params![mtime_secs, mtime_nanos, path],
                    )
                    .map(|_| ())
                    .map_err(|e: rusqlite::Error| anyhow::anyhow!(e))
            });
        }

        // Tag mismatch operations (OOB verification)
        SignalWriteOp::RecordTagMismatch {
            track_id,
            field,
            db_value,
            disk_value,
        } => {
            with_retry("record_tag_mismatch", field, || {
                db.record_tag_mismatch(*track_id, field, db_value.as_deref(), disk_value.as_deref(), &witness)
            });
        }
        SignalWriteOp::ClearTagMismatch { track_id, field } => {
            with_retry("clear_tag_mismatch", field, || {
                db.clear_tag_mismatch(*track_id, field, &witness)
            });
        }

        // =====================================================================
        // Track Index Operations (Mutation execution)
        // =====================================================================

        SignalWriteOp::IndexTrack {
            path,
            source,
            track_data,
            tags,
            scan_state,
        } => {
            with_retry("index_track", path, || {
                execute_index_track(db, path, source, track_data, tags, scan_state)
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

        SignalWriteOp::EditTrackTags { path, edits, session_id } => {
            with_retry("edit_track_tags", path, || {
                execute_edit_track_tags(db, path, edits, session_id)
            });
        }

        SignalWriteOp::LogTagEdit {
            path,
            tag_name,
            old_value,
            new_value,
            session_id,
        } => {
            with_retry("log_tag_edit", path, || {
                execute_log_tag_edit(db, path, tag_name, old_value.as_deref(), new_value.as_deref(), session_id)
            });
        }

        SignalWriteOp::UpdateTrackTag { path, tag_name, new_value } => {
            with_retry("update_track_tag", path, || {
                execute_update_track_tag(db, path, tag_name, new_value)
            });
        }

        SignalWriteOp::DeleteTrackTag { path, tag_name } => {
            with_retry("delete_track_tag", path, || {
                execute_delete_track_tag(db, path, tag_name)
            });
        }

        SignalWriteOp::UpdateTrackMetadata { path, track_data, tags } => {
            with_retry("update_track_metadata", path, || {
                execute_update_track_metadata(db, path, track_data, tags)
            });
        }

        SignalWriteOp::UpsertScanState { path, source, scan_state } => {
            with_retry("upsert_scan_state", path, || {
                execute_upsert_scan_state(db, path, source, scan_state)
            });
        }

        SignalWriteOp::DeleteScanStateByInode { source, inode } => {
            with_retry("delete_scan_state_by_inode", source, || {
                db.delete_scan_state_by_inode(source, *inode, &witness).map(|_| ())
            });
        }

        SignalWriteOp::UpdateScanStatePath { source, inode, new_path } => {
            with_retry("update_scan_state_path", new_path, || {
                db.update_scan_state_path(source, *inode, new_path, &witness)
            });
        }

        SignalWriteOp::CleanupStaleScanState { source, valid_inodes } => {
            with_retry("cleanup_stale_scan_state", source, || {
                let valid_set: std::collections::HashSet<i64> = valid_inodes.iter().copied().collect();
                db.cleanup_stale_scan_state(source, &valid_set, &witness).map(|_| ())
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

/// Get track_id by path. Returns None if track doesn't exist.
fn get_track_id_by_path(db: &Database, path: &str) -> anyhow::Result<Option<i64>> {
    use rusqlite::params;
    db.conn()
        .query_row(
            "SELECT id FROM tracks WHERE path = ?1",
            params![path],
            |row| row.get(0),
        )
        .optional()
        .map_err(|e: rusqlite::Error| anyhow::anyhow!(e))
}

/// Execute IndexTrack: atomically insert/replace track, set tags, upsert scan_state.
fn execute_index_track(
    db: &Database,
    path: &str,
    source: &str,
    track_data: &TrackData,
    tags: &[(String, String)],
    scan_state: &ScanStateData,
) -> anyhow::Result<()> {
    use rusqlite::params;

    // Convert fingerprint to BLOB if present
    let fp_blob: Option<Vec<u8>> = track_data.fingerprint.as_ref().map(|fp| fingerprint_to_blob(fp));

    // Insert or replace track
    db.conn().execute(
        r#"
        INSERT OR REPLACE INTO tracks
        (path, source, inode, file_size, file_type, duration_ms, bitrate_kbps, sample_rate, fingerprint)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
        "#,
        params![
            path,
            source,
            track_data.inode,
            track_data.file_size,
            &track_data.file_type,
            track_data.duration_ms,
            track_data.bitrate_kbps,
            track_data.sample_rate,
            &fp_blob,
        ],
    )?;

    let track_id = db.conn().last_insert_rowid();

    // Delete existing tags and insert new ones
    db.conn().execute(
        "DELETE FROM track_tags WHERE track_id = ?1",
        params![track_id],
    )?;

    for (name, value) in tags {
        if !value.is_empty() {
            db.conn().execute(
                "INSERT INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                params![track_id, name, value],
            )?;
        }
    }

    // Upsert scan_state
    db.conn().execute(
        r#"
        INSERT INTO scan_state (source, inode, path, mtime_secs, mtime_nanos, file_size)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(source, inode) DO UPDATE SET
            path = excluded.path,
            mtime_secs = excluded.mtime_secs,
            mtime_nanos = excluded.mtime_nanos,
            file_size = excluded.file_size,
            scanned_at = CURRENT_TIMESTAMP
        "#,
        params![
            source,
            scan_state.inode,
            path,
            scan_state.mtime_secs,
            scan_state.mtime_nanos,
            scan_state.file_size,
        ],
    )?;

    Ok(())
}

/// Execute DropFromIndex: delete track and cascades.
fn execute_drop_from_index(db: &Database, path: &str) -> anyhow::Result<()> {
    use rusqlite::params;

    // Get track_id first
    let track_id = match get_track_id_by_path(db, path)? {
        Some(id) => id,
        None => return Ok(()), // Track doesn't exist, nothing to do
    };

    // Delete tag_edit_history first (plain FK without CASCADE)
    db.conn().execute(
        "DELETE FROM tag_edit_history WHERE track_id = ?1",
        params![track_id],
    )?;

    // Delete the track (track_tags, tag_mismatches cascade automatically)
    db.conn().execute(
        "DELETE FROM tracks WHERE id = ?1",
        params![track_id],
    )?;

    // Also clean up scan_state entry for this path
    db.conn().execute(
        "DELETE FROM scan_state WHERE path = ?1",
        params![path],
    )?;

    Ok(())
}

/// Execute UpdateTrackPath: update path for relocated file.
fn execute_update_track_path(db: &Database, old_path: &str, new_path: &str) -> anyhow::Result<()> {
    use rusqlite::params;

    // Update tracks table
    db.conn().execute(
        "UPDATE tracks SET path = ?1 WHERE path = ?2",
        params![new_path, old_path],
    )?;

    // Update scan_state table
    db.conn().execute(
        "UPDATE scan_state SET path = ?1 WHERE path = ?2",
        params![new_path, old_path],
    )?;

    Ok(())
}

/// Execute UpdateTrackInode: update inode for replaced file.
fn execute_update_track_inode(db: &Database, path: &str, new_inode: i64) -> anyhow::Result<()> {
    use rusqlite::params;

    db.conn().execute(
        "UPDATE tracks SET inode = ?1 WHERE path = ?2",
        params![new_inode, path],
    )?;

    Ok(())
}

/// Execute SetTrackTags: replace all tags for a track.
fn execute_set_track_tags(db: &Database, path: &str, tags: &[(String, String)]) -> anyhow::Result<()> {
    use rusqlite::params;

    let track_id = get_track_id_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", path))?;

    // Delete existing tags
    db.conn().execute(
        "DELETE FROM track_tags WHERE track_id = ?1",
        params![track_id],
    )?;

    // Insert new tags
    for (name, value) in tags {
        if !value.is_empty() {
            db.conn().execute(
                "INSERT INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                params![track_id, name, value],
            )?;
        }
    }

    Ok(())
}

/// Execute EditTrackTags: apply surgical edits with history logging.
fn execute_edit_track_tags(
    db: &Database,
    path: &str,
    edits: &[TagEditOp],
    session_id: &str,
) -> anyhow::Result<()> {
    use rusqlite::params;

    let track_id = get_track_id_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", path))?;

    for edit in edits {
        // Log the edit to history
        db.conn().execute(
            "INSERT INTO tag_edit_history (track_id, field_name, old_value, new_value, session_id)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![track_id, &edit.tag_name, &edit.old_value, &edit.new_value, session_id],
        )?;

        // Apply the edit
        match (&edit.old_value, &edit.new_value) {
            (Some(old), Some(new)) => {
                // Update: delete old value, insert new
                db.conn().execute(
                    "DELETE FROM track_tags WHERE track_id = ?1 AND tag_name = ?2 AND tag_value = ?3",
                    params![track_id, &edit.tag_name, old],
                )?;
                if !new.is_empty() {
                    db.conn().execute(
                        "INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                        params![track_id, &edit.tag_name, new],
                    )?;
                }
            }
            (Some(old), None) => {
                // Delete
                db.conn().execute(
                    "DELETE FROM track_tags WHERE track_id = ?1 AND tag_name = ?2 AND tag_value = ?3",
                    params![track_id, &edit.tag_name, old],
                )?;
            }
            (None, Some(new)) => {
                // Insert
                if !new.is_empty() {
                    db.conn().execute(
                        "INSERT OR IGNORE INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                        params![track_id, &edit.tag_name, new],
                    )?;
                }
            }
            (None, None) => {
                // No-op
            }
        }
    }

    Ok(())
}

/// Execute LogTagEdit: log a single tag edit to history.
fn execute_log_tag_edit(
    db: &Database,
    path: &str,
    tag_name: &str,
    old_value: Option<&str>,
    new_value: Option<&str>,
    session_id: &str,
) -> anyhow::Result<()> {
    use rusqlite::params;

    let track_id = get_track_id_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", path))?;

    db.conn().execute(
        "INSERT INTO tag_edit_history (track_id, field_name, old_value, new_value, session_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![track_id, tag_name, old_value, new_value, session_id],
    )?;

    Ok(())
}

/// Execute UpdateTrackTag: update a single tag value.
fn execute_update_track_tag(
    db: &Database,
    path: &str,
    tag_name: &str,
    new_value: &str,
) -> anyhow::Result<()> {
    use rusqlite::params;

    let track_id = get_track_id_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", path))?;

    // Delete existing value for this tag name
    db.conn().execute(
        "DELETE FROM track_tags WHERE track_id = ?1 AND tag_name = ?2",
        params![track_id, tag_name],
    )?;

    // Insert new value if non-empty
    if !new_value.is_empty() {
        db.conn().execute(
            "INSERT INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
            params![track_id, tag_name, new_value],
        )?;
    }

    Ok(())
}

/// Execute DeleteTrackTag: delete a specific tag.
fn execute_delete_track_tag(db: &Database, path: &str, tag_name: &str) -> anyhow::Result<()> {
    use rusqlite::params;

    let track_id = get_track_id_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", path))?;

    db.conn().execute(
        "DELETE FROM track_tags WHERE track_id = ?1 AND tag_name = ?2",
        params![track_id, tag_name],
    )?;

    Ok(())
}

/// Execute UpdateTrackMetadata: full replace for out-of-band changes.
fn execute_update_track_metadata(
    db: &Database,
    path: &str,
    track_data: &TrackData,
    tags: &[(String, String)],
) -> anyhow::Result<()> {
    use rusqlite::params;

    let track_id = get_track_id_by_path(db, path)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", path))?;

    // Convert fingerprint to BLOB if present
    let fp_blob: Option<Vec<u8>> = track_data.fingerprint.as_ref().map(|fp| fingerprint_to_blob(fp));

    // Update track metadata
    db.conn().execute(
        r#"
        UPDATE tracks SET
            inode = ?1, file_size = ?2, file_type = ?3, duration_ms = ?4,
            bitrate_kbps = ?5, sample_rate = ?6, fingerprint = ?7
        WHERE id = ?8
        "#,
        params![
            track_data.inode,
            track_data.file_size,
            &track_data.file_type,
            track_data.duration_ms,
            track_data.bitrate_kbps,
            track_data.sample_rate,
            &fp_blob,
            track_id,
        ],
    )?;

    // Replace tags
    db.conn().execute(
        "DELETE FROM track_tags WHERE track_id = ?1",
        params![track_id],
    )?;

    for (name, value) in tags {
        if !value.is_empty() {
            db.conn().execute(
                "INSERT INTO track_tags (track_id, tag_name, tag_value) VALUES (?1, ?2, ?3)",
                params![track_id, name, value],
            )?;
        }
    }

    Ok(())
}

/// Execute UpsertScanState: upsert scan state entry.
fn execute_upsert_scan_state(
    db: &Database,
    path: &str,
    source: &str,
    scan_state: &ScanStateData,
) -> anyhow::Result<()> {
    use rusqlite::params;

    db.conn().execute(
        r#"
        INSERT INTO scan_state (source, inode, path, mtime_secs, mtime_nanos, file_size)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(source, inode) DO UPDATE SET
            path = excluded.path,
            mtime_secs = excluded.mtime_secs,
            mtime_nanos = excluded.mtime_nanos,
            file_size = excluded.file_size,
            scanned_at = CURRENT_TIMESTAMP
        "#,
        params![
            source,
            scan_state.inode,
            path,
            scan_state.mtime_secs,
            scan_state.mtime_nanos,
            scan_state.file_size,
        ],
    )?;

    Ok(())
}

/// Execute ClearTagMismatchesForTrack: clear all mismatches for a track.
fn execute_clear_tag_mismatches_for_track(db: &Database, path: &str) -> anyhow::Result<()> {
    use rusqlite::params;

    let track_id = match get_track_id_by_path(db, path)? {
        Some(id) => id,
        None => return Ok(()), // Track doesn't exist, nothing to clear
    };

    db.conn().execute(
        "DELETE FROM tag_mismatches WHERE track_id = ?1",
        params![track_id],
    )?;

    Ok(())
}

use rusqlite::OptionalExtension;
