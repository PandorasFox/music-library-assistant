//! Global performance config (OnceLock + accessor functions).

use super::types::PerformanceOpinions;
use std::sync::OnceLock;

static PERFORMANCE_CONFIG: OnceLock<PerformanceOpinions> = OnceLock::new();

/// Initialize the global performance config. Called once at startup.
pub fn init_performance_config(opinions: PerformanceOpinions) {
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
pub fn get_db_cache_kb() -> i64 {
    let mb = PERFORMANCE_CONFIG
        .get()
        .map(|p| p.db_cache_mb)
        .unwrap_or(256);

    // Convert MB to KB, return as negative (SQLite convention for KB)
    -(mb as i64 * 1024)
}

/// Check if timing instrumentation is enabled.
/// When false, stats collection is skipped entirely for better performance.
pub fn is_timing_enabled() -> bool {
    PERFORMANCE_CONFIG
        .get()
        .map(|p| p.timing_instrumentation)
        .unwrap_or(false)
}
