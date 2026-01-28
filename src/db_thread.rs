//! Dedicated DB write thread for eliminating connection contention.
//!
//! Provides fire-and-forget write operations via typed channels, preserving
//! witness semantics at call sites. The thread owns a single write connection
//! and processes operations sequentially, eliminating lock contention.
//!
//! ## Architecture
//!
//! - `SignalWriteSender`: For health signal operations (requires `SignalWitness`)
//! - `IndexWriteSender`: For index mutations (requires `MutationExecutionWitness`) - Phase 2
//! - `DbThreadHandle`: For stats access and shutdown coordination
//!
//! ## Witness Semantics
//!
//! Witnesses are required at the *send* call site, not at execution time.
//! This preserves the invariant that only legitimate execution contexts can
//! enqueue writes, even though the actual DB operation happens asynchronously.

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
    // Shutdown
    // =========================================================================

    /// Shutdown sentinel — close DB connection and exit thread.
    Shutdown,
}

/// Index write operations (track mutations) - Phase 2 placeholder.
#[derive(Debug)]
enum IndexWriteOp {
    // Phase 2: Will mirror mutation types from corpus/mutations/
    Placeholder,
}

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
        _witness: &ComputationWitness,
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
        _witness: &ComputationWitness,
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
}

/// Sender for index write operations (Phase 2 placeholder).
///
/// Clone-able, thread-safe. All methods will require `MutationExecutionWitness`.
#[derive(Clone)]
pub struct IndexWriteSender {
    _tx: Sender<IndexWriteOp>,
    _stats: Arc<SharedStats>,
}

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

    // Create channels (unbounded)
    let (signal_tx, signal_rx) = mpsc::channel::<SignalWriteOp>();
    let (index_tx, index_rx) = mpsc::channel::<IndexWriteOp>();

    let thread_stats = Arc::clone(&stats);
    let thread_handle = thread::spawn(move || {
        run_db_thread(signal_rx, index_rx, thread_stats);
    });

    let handle = DbThreadHandle {
        stats: Arc::clone(&stats),
        thread_handle: Some(thread_handle),
    };

    let signal_sender = SignalWriteSender {
        tx: signal_tx,
        stats: Arc::clone(&stats),
    };

    // Initialize global sender (computation code accesses via signal_sender())
    let _ = SIGNAL_SENDER.set(signal_sender);

    // Index sender placeholder (Phase 2)
    let _index_sender = IndexWriteSender {
        _tx: index_tx,
        _stats: Arc::clone(&stats),
    };

    handle
}

/// Main loop for the DB write thread.
fn run_db_thread(
    signal_rx: Receiver<SignalWriteOp>,
    _index_rx: Receiver<IndexWriteOp>,
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
                db.clear_library_scan_state(library_name).map(|_| ())
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
                db.record_library_file(library_name, library_root, file_path, *inode, *scanned_at)
            });
        }

        // Bulk operations (Awake phase content analysis)
        SignalWriteOp::ClearSignalsByType { issue_type } => {
            let issue_type_str = issue_type.as_str();
            with_retry("clear_signals_by_type", issue_type_str, || {
                use rusqlite::params;
                db.conn
                    .execute(
                        "DELETE FROM signals WHERE issue_type = ?1",
                        params![issue_type_str],
                    )
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!(e))
            });
        }
        SignalWriteOp::UpdateScanStateMtime {
            path,
            mtime_secs,
            mtime_nanos,
        } => {
            with_retry("update_scan_state_mtime", path, || {
                use rusqlite::params;
                db.conn
                    .execute(
                        "UPDATE scan_state SET mtime_secs = ?1, mtime_nanos = ?2 WHERE path = ?3",
                        params![mtime_secs, mtime_nanos, path],
                    )
                    .map(|_| ())
                    .map_err(|e| anyhow::anyhow!(e))
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
                db.record_tag_mismatch(*track_id, field, db_value.as_deref(), disk_value.as_deref())
            });
        }
        SignalWriteOp::ClearTagMismatch { track_id, field } => {
            with_retry("clear_tag_mismatch", field, || {
                db.clear_tag_mismatch(*track_id, field)
            });
        }

        // Shutdown is handled in the run_db_thread loop, never reaches here
        SignalWriteOp::Shutdown => unreachable!("Shutdown handled in run_db_thread loop"),
    }
}
