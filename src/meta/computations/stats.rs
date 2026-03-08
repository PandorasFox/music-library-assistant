//! Thread-local performance statistics tracking.
//!
//! Each worker thread maintains its own stats via thread-local storage.
//! These are periodically snapshotted and aggregated by the daemon.

use std::cell::RefCell;

// ============================================================================
// Constants
// ============================================================================

/// Number of read time samples to keep per thread for median calculation
const READ_SAMPLE_SIZE: usize = 64;

// ============================================================================
// ThreadStats
// ============================================================================

/// Performance statistics accumulated per worker thread.
///
/// Each thread maintains its own stats via thread-local storage.
/// These are periodically snapshotted and aggregated by the daemon.
#[derive(Debug, Clone)]
pub struct ThreadStats {
    /// Unique identifier for this thread (assigned on first task)
    pub thread_id: u64,
    /// Total tasks completed by this thread
    pub tasks_completed: u64,
    /// Cumulative task execution time in milliseconds
    pub total_task_ms: u64,
    /// Slowest single task execution time
    pub max_task_ms: u64,
    /// Label of the slowest task
    pub max_task_label: String,
    /// Number of DB connection opens (should be 1 per thread)
    pub db_opens: u64,
    /// Cumulative time spent in DB reads (microseconds)
    pub total_db_read_us: u64,
    /// Number of DB read operations
    pub db_read_count: u64,
    /// Slowest single DB read (microseconds)
    pub max_db_read_us: u64,
    /// Circular buffer of recent read times for median calculation
    pub read_samples: [u64; READ_SAMPLE_SIZE],
    /// Write index into read_samples (wraps around)
    pub read_sample_idx: usize,
    /// How many samples have been written (caps at READ_SAMPLE_SIZE)
    pub read_sample_count: usize,
}

impl Default for ThreadStats {
    fn default() -> Self {
        Self::new()
    }
}

impl ThreadStats {
    pub const fn new() -> Self {
        Self {
            thread_id: 0,
            tasks_completed: 0,
            total_task_ms: 0,
            max_task_ms: 0,
            max_task_label: String::new(),
            db_opens: 0,
            total_db_read_us: 0,
            db_read_count: 0,
            max_db_read_us: 0,
            read_samples: [0; READ_SAMPLE_SIZE],
            read_sample_idx: 0,
            read_sample_count: 0,
        }
    }
}

// ============================================================================
// Thread-Local Storage
// ============================================================================

// Thread-local cached READ-ONLY database connection for computation workers.
// Each worker thread opens once and reuses, eliminating connection overhead.
//
// IMPORTANT: This connection has `PRAGMA query_only = ON` set.
// Any write operations (INSERT, UPDATE, DELETE) will SILENTLY FAIL.
// All writes from computations must go through `write_thread::signal_sender()`.
thread_local! {
    static THREAD_READ_ONLY_DB: RefCell<Option<crate::db::Database>> = const { RefCell::new(None) };
    pub(super) static THREAD_STATS: RefCell<ThreadStats> = const { RefCell::new(ThreadStats::new()) };
}

// ============================================================================
// Public API
// ============================================================================

/// Get a snapshot of the current thread's stats.
pub fn get_thread_stats() -> ThreadStats {
    THREAD_STATS.with(|cell| cell.borrow().clone())
}

/// Close the thread-local database connection on the current thread.
///
/// Called via `rayon::broadcast()` during shutdown to ensure all worker
/// thread connections are closed before the db_thread attempts WAL checkpoint.
pub fn close_thread_local_connection() {
    THREAD_READ_ONLY_DB.with(|cell| {
        cell.borrow_mut().take();
    });
}

/// Execute a function with the thread-local READ-ONLY database connection.
/// Opens and caches the connection on first use per thread.
/// Tracks DB read timing for performance instrumentation (when enabled).
///
/// # IMPORTANT: Read-Only Access
///
/// This connection uses `PRAGMA query_only = ON`. All write operations
/// (INSERT, UPDATE, DELETE) will fail. Computations and mutations that need
/// to persist data must route writes through `write_thread::signal_sender()`.
///
/// The `read_only_db` parameter name in executor functions reflects this.
///
/// # Usage
///
/// This function is used by both computation and mutation worker threads
/// to share a single read-only connection per thread, avoiding the overhead
/// of opening new connections for each task.
pub fn with_read_only_db<T, F>(f: F) -> Result<T, String>
where
    F: FnOnce(&crate::db::ReadOnlyDb<'_>) -> T,
{
    use crate::config;
    use crate::db::{Database, ReadOnlyDb};

    THREAD_READ_ONLY_DB.with(|cell| {
        let mut opt = cell.borrow_mut();
        if opt.is_none() {
            let db_path = config::get_db_path().map_err(|e| e.to_string())?;
            let db = Database::open_read_only(&db_path).map_err(|e| e.to_string())?;
            *opt = Some(db);
            record_db_open();
        }

        let read_only_db = ReadOnlyDb::new(opt.as_ref().unwrap());

        // Only time DB access when instrumentation is enabled
        if config::is_timing_enabled() {
            let db_start = std::time::Instant::now();
            let result = f(&read_only_db);
            record_db_read(db_start.elapsed().as_micros() as u64);
            Ok(result)
        } else {
            Ok(f(&read_only_db))
        }
    })
}

// ============================================================================
// Internal Recording Functions
// ============================================================================

/// Assign a thread ID if not already set. Returns the thread ID.
/// Only tracks when timing instrumentation is enabled.
pub(super) fn ensure_thread_id() -> u64 {
    use crate::config;
    if !config::is_timing_enabled() {
        return 0;
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static THREAD_ID_COUNTER: AtomicU64 = AtomicU64::new(1);

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        if stats.thread_id == 0 {
            stats.thread_id = THREAD_ID_COUNTER.fetch_add(1, Ordering::Relaxed);
        }
        stats.thread_id
    })
}

/// Record a completed task's stats.
/// No-op when timing instrumentation is disabled.
pub(super) fn record_task_stats(label: &str, duration_ms: u64) {
    use crate::config;
    if !config::is_timing_enabled() {
        return;
    }

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        stats.tasks_completed += 1;
        stats.total_task_ms += duration_ms;
        if duration_ms > stats.max_task_ms {
            stats.max_task_ms = duration_ms;
            stats.max_task_label = label.to_string();
        }
    });
}

/// Record a DB read operation's timing.
/// No-op when timing instrumentation is disabled.
fn record_db_read(duration_us: u64) {
    use crate::config;
    if !config::is_timing_enabled() {
        return;
    }

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        stats.total_db_read_us += duration_us;
        stats.db_read_count += 1;
        if duration_us > stats.max_db_read_us {
            stats.max_db_read_us = duration_us;
        }
        // Add to circular sample buffer
        let idx = stats.read_sample_idx;
        stats.read_samples[idx] = duration_us;
        stats.read_sample_idx = (idx + 1) % READ_SAMPLE_SIZE;
        if stats.read_sample_count < READ_SAMPLE_SIZE {
            stats.read_sample_count += 1;
        }
    });
}

/// Record a DB connection open.
/// No-op when timing instrumentation is disabled.
fn record_db_open() {
    use crate::config;
    if !config::is_timing_enabled() {
        return;
    }

    THREAD_STATS.with(|cell| {
        let mut stats = cell.borrow_mut();
        stats.db_opens += 1;
    });
}
