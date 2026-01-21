//! Dedicated DB write thread for eliminating connection contention.
//!
//! Provides fire-and-forget write operations via typed channels, preserving
//! witness semantics at call sites. The thread owns a single write connection
//! and processes operations sequentially, eliminating lock contention.
//!
//! ## Architecture
//!
//! - `SignalWriteSender`: For health signal operations (requires `ComputationWitness`)
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
    AggregateSignal, AggregateSignalType, FileSignalType, HealthIssue, HealthIssueType,
};
use crate::corpus::db::Database;
use crate::config;

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

// ============================================================================
// Message Types
// ============================================================================

/// Signal write operations (health signals).
#[derive(Debug)]
enum SignalWriteOp {
    /// File signal (no metadata) - type-safe, preferred
    EnsureFileSignal {
        signal_type: FileSignalType,
        path: String,
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
///
/// Note: `thread_handle` is never accessed but must be retained - dropping it kills the thread.
pub struct DbThreadHandle {
    stats: Arc<SharedStats>,
    _thread_handle: JoinHandle<()>,
}

impl DbThreadHandle {
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
    // =========================================================================
    // Type-safe file signal operations (preferred)
    // =========================================================================

    /// Enqueue a file signal (idempotent create, no metadata).
    pub fn ensure_file_signal(
        &self,
        signal_type: FileSignalType,
        path: &str,
        _witness: &ComputationWitness,
    ) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
        let _ = self.tx.send(SignalWriteOp::EnsureFileSignal {
            signal_type,
            path: path.to_string(),
        });
    }

    /// Clear a file signal (idempotent delete).
    pub fn clear_file_signal(
        &self,
        signal_type: FileSignalType,
        path: &str,
        _witness: &ComputationWitness,
    ) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
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
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
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
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
        let _ = self.tx.send(SignalWriteOp::EnsureAggregateSignal {
            signal_type,
            key: key.to_string(),
            metadata_json: metadata_json.map(|s| s.to_string()),
        });
    }

    /// Replace an aggregate signal (delete + insert).
    pub fn replace_aggregate_signal(&self, signal: AggregateSignal, _witness: &ComputationWitness) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
        let _ = self.tx.send(SignalWriteOp::ReplaceAggregateSignal { signal });
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
        _thread_handle: thread_handle,
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
            let _ = config::log_message(&format!("[DB_THREAD] Failed to open database: {}", e));
            return;
        }
    };

    let _ = config::log_message("[DB_THREAD] Started");

    // Process signal operations
    // Note: Using recv() which blocks until a message arrives or channel closes
    let timing_enabled = stats.timing_enabled;

    loop {
        match signal_rx.recv() {
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
                let _ = config::log_message("[DB_THREAD] Channel closed, exiting");
                break;
            }
        }
    }
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
                        let _ = config::log_message(&format!(
                            "[DB_THREAD] {} retry {}/{} after {}ms: {} | {}",
                            op_name, attempt + 1, MAX_RETRIES, delay, reason, context
                        ));
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    } else {
                        let _ = config::log_message(&format!(
                            "[DB_THREAD] {} FAILED after {} retries: {} | {}",
                            op_name, MAX_RETRIES, reason, context
                        ));
                        return;
                    }
                } else {
                    // Non-retryable error
                    let _ = config::log_message(&format!(
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

    }
}
