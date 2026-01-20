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
    /// Clear signals in a directory (for file signals)
    ClearFileSignalsInDirectory {
        directory: PathBuf,
        signal_type: FileSignalType,
    },
    // Legacy operations (for migration)
    #[allow(dead_code)]
    EnsureSignal {
        issue_type: HealthIssueType,
        issue_key: String,
        metadata_json: Option<String>,
    },
    ClearSignal {
        issue_type: HealthIssueType,
        issue_key: String,
    },
    ReplaceSignal {
        issue: HealthIssue,
    },
    ClearSignalsInDirectory {
        directory: PathBuf,
        issue_type: HealthIssueType,
    },
}

/// Index write operations (track mutations) - Phase 2 placeholder.
#[derive(Debug)]
#[allow(dead_code)]
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
struct SharedStats {
    /// Total write operations processed
    total_writes: AtomicU64,
    /// Signal operations (ensure/clear/replace)
    signal_writes: AtomicU64,
    /// Index operations (Phase 2)
    index_writes: AtomicU64,
    /// Current queue depth (approximate)
    queue_depth: AtomicU64,
    /// Cumulative microseconds spent in DB operations
    total_db_time_us: AtomicU64,
    /// True when queue is empty (for shutdown blocking)
    queue_empty: AtomicBool,
    /// Thread start time for rate calculations
    start_time: Instant,
}

impl SharedStats {
    fn new() -> Self {
        Self {
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
    #[allow(dead_code)]
    thread_handle: JoinHandle<()>,
}

impl DbThreadHandle {
    /// Check if the write queue is empty (for shutdown blocking).
    pub fn queue_empty(&self) -> bool {
        self.stats.queue_empty.load(Ordering::Acquire)
    }

    /// Get current stats snapshot for UI display.
    pub fn stats(&self) -> DbThreadStats {
        let total_writes = self.stats.total_writes.load(Ordering::Relaxed);
        let total_db_time_us = self.stats.total_db_time_us.load(Ordering::Relaxed);
        let elapsed_secs = self.stats.start_time.elapsed().as_secs_f64();

        DbThreadStats {
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
        }
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

    // =========================================================================
    // Legacy operations (for migration compatibility)
    // =========================================================================

    /// Legacy: Enqueue an ensure_signal operation.
    #[deprecated(note = "Use ensure_file_signal or ensure_aggregate_signal instead")]
    pub fn ensure_signal(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
        metadata_json: Option<&str>,
        _witness: &ComputationWitness,
    ) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
        let _ = self.tx.send(SignalWriteOp::EnsureSignal {
            issue_type,
            issue_key: issue_key.to_string(),
            metadata_json: metadata_json.map(|s| s.to_string()),
        });
    }

    /// Legacy: Enqueue a clear_signal operation.
    #[deprecated(note = "Use clear_file_signal instead")]
    pub fn clear_signal(
        &self,
        issue_type: HealthIssueType,
        issue_key: &str,
        _witness: &ComputationWitness,
    ) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
        let _ = self.tx.send(SignalWriteOp::ClearSignal {
            issue_type,
            issue_key: issue_key.to_string(),
        });
    }

    /// Legacy: Enqueue a replace_signal operation.
    #[deprecated(note = "Use replace_aggregate_signal instead")]
    pub fn replace_signal(&self, issue: HealthIssue, _witness: &ComputationWitness) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
        let _ = self.tx.send(SignalWriteOp::ReplaceSignal { issue });
    }

    /// Legacy: Enqueue a clear_signals_in_directory operation.
    #[deprecated(note = "Use clear_file_signals_in_directory instead")]
    pub fn clear_signals_in_directory(
        &self,
        directory: &std::path::Path,
        issue_type: HealthIssueType,
        _witness: &ComputationWitness,
    ) {
        self.stats.queue_depth.fetch_add(1, Ordering::Relaxed);
        self.stats.queue_empty.store(false, Ordering::Release);
        let _ = self.tx.send(SignalWriteOp::ClearSignalsInDirectory {
            directory: directory.to_path_buf(),
            issue_type,
        });
    }
}

/// Sender for index write operations (Phase 2 placeholder).
///
/// Clone-able, thread-safe. All methods will require `MutationExecutionWitness`.
#[derive(Clone)]
pub struct IndexWriteSender {
    #[allow(dead_code)]
    tx: Sender<IndexWriteOp>,
    #[allow(dead_code)]
    stats: Arc<SharedStats>,
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

    // Create channels (unbounded)
    let (signal_tx, signal_rx) = mpsc::channel::<SignalWriteOp>();
    let (index_tx, index_rx) = mpsc::channel::<IndexWriteOp>();

    let thread_stats = Arc::clone(&stats);
    let thread_handle = thread::spawn(move || {
        run_db_thread(signal_rx, index_rx, thread_stats);
    });

    let handle = DbThreadHandle {
        stats: Arc::clone(&stats),
        thread_handle,
    };

    let signal_sender = SignalWriteSender {
        tx: signal_tx,
        stats: Arc::clone(&stats),
    };

    // Initialize global sender (computation code accesses via signal_sender())
    let _ = SIGNAL_SENDER.set(signal_sender);

    // Index sender placeholder (Phase 2)
    let _index_sender = IndexWriteSender {
        tx: index_tx,
        stats: Arc::clone(&stats),
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
    loop {
        match signal_rx.recv() {
            Ok(op) => {
                let start = Instant::now();
                execute_signal_op(&db, &op);
                let elapsed_us = start.elapsed().as_micros() as u64;

                // Update stats
                stats.total_writes.fetch_add(1, Ordering::Relaxed);
                stats.signal_writes.fetch_add(1, Ordering::Relaxed);
                stats.total_db_time_us.fetch_add(elapsed_us, Ordering::Relaxed);
                stats.queue_depth.fetch_sub(1, Ordering::Relaxed);

                // Check if queue is now empty
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

/// Execute a single signal write operation.
#[allow(deprecated)]
fn execute_signal_op(db: &Database, op: &SignalWriteOp) {
    // Note: We don't have a ComputationWitness here, but we need one for the db methods.
    // The witness was checked at the send site. We use a thread-local witness for execution.
    let witness = crate::corpus::computations::ComputationWitness::new_for_db_thread();

    match op {
        // Type-safe file signal operations
        SignalWriteOp::EnsureFileSignal { signal_type, path } => {
            if let Err(e) = db.ensure_file_signal(*signal_type, path, &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] ensure_file_signal failed: {}",
                    e
                ));
            }
        }
        SignalWriteOp::ClearFileSignal { signal_type, path } => {
            if let Err(e) = db.clear_file_signal(*signal_type, path, &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] clear_file_signal failed: {}",
                    e
                ));
            }
        }
        SignalWriteOp::ClearFileSignalsInDirectory {
            directory,
            signal_type,
        } => {
            if let Err(e) = db.clear_file_signals_in_directory(directory, *signal_type, &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] clear_file_signals_in_directory failed: {}",
                    e
                ));
            }
        }

        // Aggregate signal operations
        SignalWriteOp::EnsureAggregateSignal {
            signal_type,
            key,
            metadata_json,
        } => {
            if let Err(e) = db.ensure_aggregate_signal(*signal_type, key, metadata_json.as_deref(), &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] ensure_aggregate_signal failed: {}",
                    e
                ));
            }
        }
        SignalWriteOp::ReplaceAggregateSignal { signal } => {
            if let Err(e) = db.replace_aggregate_signal(signal, &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] replace_aggregate_signal failed: {}",
                    e
                ));
            }
        }

        // Legacy operations
        SignalWriteOp::EnsureSignal {
            issue_type,
            issue_key,
            metadata_json,
        } => {
            if let Err(e) = db.ensure_signal(*issue_type, issue_key, metadata_json.as_deref(), &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] ensure_signal failed: {}",
                    e
                ));
            }
        }
        SignalWriteOp::ClearSignal {
            issue_type,
            issue_key,
        } => {
            if let Err(e) = db.clear_signal(*issue_type, issue_key, &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] clear_signal failed: {}",
                    e
                ));
            }
        }
        SignalWriteOp::ReplaceSignal { issue } => {
            if let Err(e) = db.replace_signal(issue, &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] replace_signal failed: {}",
                    e
                ));
            }
        }
        SignalWriteOp::ClearSignalsInDirectory {
            directory,
            issue_type,
        } => {
            if let Err(e) = db.clear_signals_in_directory(directory, *issue_type, &witness) {
                let _ = config::log_message(&format!(
                    "[DB_THREAD] clear_signals_in_directory failed: {}",
                    e
                ));
            }
        }
    }
}
