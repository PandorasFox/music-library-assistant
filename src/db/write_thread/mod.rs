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

mod executor;
pub mod types;
mod sender;

pub use sender::SignalWriteSender;
pub use types::*;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, OnceLock};
use std::thread::{self, JoinHandle};

use crate::config;
use crate::corpus::tags::TagSet;
use crate::db::Database;

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

/// Get the global signal sender, returning an error if the DB thread
/// hasn't been initialized yet.
///
/// This is the preferred accessor for mutation executors and tag operations
/// where the DB thread being absent is an unrecoverable error.
pub fn require_sender() -> anyhow::Result<&'static SignalWriteSender> {
    signal_sender().ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))
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
    let sender = SIGNAL_SENDER
        .get()
        .ok_or_else(|| "db_thread not initialized".to_string())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    sender.mark_enqueued();
    let _ = sender.tx.send(DbWriteOp::ExecuteVacuum { result_tx: tx });
    rx.recv()
        .map_err(|_| "db_thread disconnected during VACUUM".to_string())?
}

/// Apply schema reconciliation on the db_thread's write connection.
///
/// Blocks the caller until reconciliation completes. Runs on the db_thread
/// which owns the write connection.
pub fn execute_reconciliation() -> Result<(), String> {
    let sender = SIGNAL_SENDER
        .get()
        .ok_or_else(|| "db_thread not initialized".to_string())?;
    let (tx, rx) = std::sync::mpsc::sync_channel(1);
    sender.mark_enqueued();
    let _ = sender
        .tx
        .send(DbWriteOp::ApplyReconciliation { result_tx: tx });
    rx.recv()
        .map_err(|_| "db_thread disconnected during reconciliation".to_string())?
}

/// Update the SQLite PRAGMA cache_size on the write thread's connection.
///
/// Fire-and-forget: enqueues the command and returns immediately.
pub fn set_cache_size(kb: i64) {
    if let Some(sender) = SIGNAL_SENDER.get() {
        sender.mark_enqueued();
        let _ = sender.tx.send(DbWriteOp::SetCacheSize { kb });
    }
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
    /// Truncate an entire signal table (DELETE FROM).
    ClearSignalTable {
        clear_fn: fn(&rusqlite::Connection) -> rusqlite::Result<usize>,
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
        signal: crate::meta::signals::registry::TypedSignalWrite,
    },
    /// Write a batch of typed signals in a single transaction.
    ///
    /// Reduces channel overhead for reconcile operations that emit many signals.
    WriteTypedSignalBatch {
        signals: Vec<crate::meta::signals::registry::TypedSignalWrite>,
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

    /// Index an image file entry in the files table during scanning.
    /// Like IndexDirectory but for image files (is_dir=0).
    IndexImageFile {
        path: String,
        zone: String,
        inode: i64,
        mtime_secs: i64,
        mtime_nanos: i64,
        file_size: i64,
    },

    /// Upsert image metadata into the image_info table.
    UpsertImageInfo {
        inode: i64,
        format: String,
        width: u32,
        height: u32,
        role: String,
    },

    /// No-op vestige: tag_mismatches table was dropped. OOB signals handle conflicts now.
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

    /// Mark a batch of inodes dirty for a specific computation type.
    MarkDirtyInodes {
        inodes: Vec<i64>,
        computation_type: String,
    },

    // =========================================================================
    // External Matching Operations (AcoustID fetch thread results)
    // =========================================================================
    /// Insert an external match result.
    InsertExternalMatch {
        inode: i64,
        fingerprint: Vec<u8>,
        source: i64,
        recording_id: String,
        confidence: f64,
        raw_response: Option<Vec<u8>>,
        fetched_at: i64,
    },

    /// Insert a no-match result for a fingerprint.
    InsertExternalNoMatch {
        fingerprint: Vec<u8>,
        source: i64,
        queried_at: i64,
    },

    /// Upsert an external retry entry.
    UpsertExternalRetry {
        inode: i64,
        fingerprint: Vec<u8>,
        source: i64,
        error: String,
    },

    /// Delete an external retry entry (after successful lookup).
    DeleteExternalRetry {
        inode: i64,
        source: i64,
    },

    /// Drop all external match data for an inode.
    DropExternalMatch {
        inode: i64,
    },

    // =========================================================================
    // MusicBrainz Cache Operations (MB fetch thread results — no witness needed)
    // =========================================================================
    /// Upsert a MusicBrainz cache entry (recording, artist, or release).
    UpsertMbCache {
        table: &'static str,
        id_col: &'static str,
        id: String,
        raw_json: Vec<u8>,
        fetched_at: i64,
    },

    /// Insert a known MusicBrainz entity (for resumable fetching).
    InsertMbKnownEntity {
        mbid: String,
        entity_type: String,
        discovered_from: Option<String>,
        discovered_at: i64,
    },

    // =========================================================================
    // Edit History Purge Operations (operator-confirmed UI action)
    // =========================================================================
    /// Delete all rows from tag_edit_history.
    ClearTagEditHistory,

    /// Delete all rows from tag_edit_history for a single session.
    ClearTagEditHistorySession {
        session_id: String,
    },

    // =========================================================================
    // Shutdown
    // =========================================================================
    /// Execute VACUUM on the write connection. Handled in main loop (like Shutdown).
    ExecuteVacuum {
        result_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
    },

    /// Apply schema reconciliation on the write connection. Handled in main loop (like Shutdown).
    ApplyReconciliation {
        result_tx: std::sync::mpsc::SyncSender<Result<(), String>>,
    },

    /// Shutdown sentinel — close DB connection and exit thread.
    // =========================================================================
    // Release Packing Pipeline Operations (intermediate tables)
    // =========================================================================

    /// Truncate both release packing intermediate tables for a fresh pipeline run.
    TruncatePackingTables,

    /// Write a batch of rows to release_packing_manifest.
    WritePackingManifest {
        rows: Vec<(String, i32, String, String, i32)>, // (release_id, total_tracks, title, artist, media_count)
    },

    /// Write a batch of scored candidates to release_packing_scores.
    WritePackingScores {
        rows: Vec<PackingScoreRow>,
    },

    /// Write a batch of candidate rows to release_packing_candidates.
    WritePackingCandidates {
        rows: Vec<PackingCandidateRow>,
    },

    /// Write pending AcoustID submissions (elimination matching results).
    WritePendingAcoustIdSubmissions {
        rows: Vec<PendingAcoustIdSubmission>,
    },

    // =========================================================================
    // Runtime Config Updates
    // =========================================================================

    /// Update SQLite PRAGMA cache_size on the write connection.
    /// Sent by Witch when performance config changes at runtime.
    SetCacheSize { kb: i64 },

    Shutdown,
}

// ============================================================================
// Shared Stats (Atomic Counters)
// ============================================================================

/// Shared statistics between DB thread and handle.
///
/// All fields are atomic for lock-free access from UI thread.
struct SharedStats {
    /// Current queue depth - tracked for shutdown coordination and UI display.
    queue_depth: AtomicU64,
    /// True when queue is empty - tracked for shutdown blocking.
    queue_empty: AtomicBool,
}

impl SharedStats {
    fn new() -> Self {
        Self {
            queue_depth: AtomicU64::new(0),
            queue_empty: AtomicBool::new(true),
        }
    }
}

/// Handle to the DB thread for stats access and shutdown coordination.
pub struct DbThreadHandle {
    stats: Arc<SharedStats>,
    thread_handle: Option<JoinHandle<()>>,
}

impl DbThreadHandle {
    /// Check if the write queue is empty (for shutdown blocking).
    pub fn queue_empty(&self) -> bool {
        self.stats.queue_empty.load(Ordering::Acquire)
    }

    /// Get the current queue depth.
    pub fn queue_depth(&self) -> u64 {
        self.stats.queue_depth.load(Ordering::Relaxed)
    }
}

impl crate::witch::types::ManagedThread for DbThreadHandle {
    fn send_shutdown(&self) {
        request_shutdown();
    }

    fn take_handle(&mut self) -> Option<JoinHandle<()>> {
        self.thread_handle.take()
    }
}

// ============================================================================
// DB Thread Implementation
// ============================================================================

/// Spawn the DB write thread.
///
/// Returns the handle for stats/shutdown. Also initializes the global signal sender
/// so computation code can access it via `signal_sender()`.
pub fn spawn() -> DbThreadHandle {
    let stats = Arc::new(SharedStats::new());

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
fn run_db_thread(signal_rx: Receiver<DbWriteOp>, stats: Arc<SharedStats>) {
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
    loop {
        match signal_rx.recv() {
            Ok(DbWriteOp::ExecuteVacuum { result_tx }) => {
                crate::logging::log_general("[DB_THREAD] Executing VACUUM");
                let result = db
                    .conn()
                    .execute_batch("VACUUM")
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
            Ok(DbWriteOp::ApplyReconciliation { result_tx }) => {
                crate::logging::log_general("[DB_THREAD] Applying schema reconciliation");
                let witness = crate::witch::MaintenanceWitness::new_for_db_thread();
                let result = crate::db::reconciler::ReconciliationPlan::compute_full(&db)
                    .and_then(|plan| plan.execute(&db, &witness))
                    .map_err(|e| format!("{:#}", e));
                if result.is_ok() {
                    crate::logging::log_general("[DB_THREAD] Schema reconciliation completed");
                }
                let _ = result_tx.send(result);
                stats.queue_depth.fetch_sub(1, Ordering::Relaxed);
                if stats.queue_depth.load(Ordering::Relaxed) == 0 {
                    stats.queue_empty.store(true, Ordering::Release);
                }
            }
            Ok(DbWriteOp::SetCacheSize { kb }) => {
                let _ = db.conn().execute_batch(&format!("PRAGMA cache_size = {};", kb));
                crate::logging::log_general(format!(
                    "[DB_THREAD] Updated cache_size to {} KB",
                    kb
                ));
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
                executor::execute_signal_op(&db, &op);

                // Update queue management (needed for shutdown coordination)
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
