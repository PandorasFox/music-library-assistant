//! Global performance config (OnceLock + accessor functions).

use super::types::PerformanceOpinions;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::OnceLock;

static PERFORMANCE_CONFIG: OnceLock<PerformanceOpinions> = OnceLock::new();

/// Dynamic cache_kb value. Initialized from OnceLock at startup,
/// updated at runtime by Hades when config changes arrive.
static CACHE_KB: AtomicI64 = AtomicI64::new(0);

/// Whether CACHE_KB has been initialized (0 is a valid sentinel since
/// the real value is always negative).
static CACHE_KB_INITIALIZED: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// Initialize the global performance config. Called once at startup.
pub fn init_performance_config(opinions: PerformanceOpinions) {
    // Initialize the atomic cache_kb from opinions
    let cache_kb = -(opinions.db_cache_mb as i64 * 1024);
    CACHE_KB.store(cache_kb, Ordering::Release);
    CACHE_KB_INITIALIZED.store(true, Ordering::Release);

    let _ = PERFORMANCE_CONFIG.set(opinions);
}

/// Get the configured worker thread count.
/// Returns 2x logical cores if not configured or not initialized.
pub fn get_worker_thread_count() -> usize {
    let default_count = std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8);

    PERFORMANCE_CONFIG
        .get()
        .and_then(|p| p.worker_threads)
        .unwrap_or(default_count)
}

/// Get the configured DB cache size in KB (for SQLite PRAGMA cache_size).
/// Returns negative value as SQLite interprets negative as KB.
///
/// Reads from the atomic value, which is updated dynamically by Hades
/// when config changes arrive at runtime.
pub fn get_db_cache_kb() -> i64 {
    if CACHE_KB_INITIALIZED.load(Ordering::Acquire) {
        CACHE_KB.load(Ordering::Acquire)
    } else {
        // Fallback before init — 256 MB default
        -(256_i64 * 1024)
    }
}

/// Update the dynamic cache_kb value at runtime.
///
/// Called by Hades when a config update arrives. Thread-local connections
/// will pick up the new value lazily on their next task execution.
pub fn set_db_cache_kb(kb: i64) {
    CACHE_KB.store(kb, Ordering::Release);
    CACHE_KB_INITIALIZED.store(true, Ordering::Release);
}
