//! Thread-safe worker performance statistics.
//!
//! This module is part of the Witch subsystem. See `witch/mod.rs` for overview.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::types::{TaskResult, WorkerStats};

// ============================================================================
// Shared Worker Stats (Thread-Safe)
// ============================================================================

/// Inner data for SharedWorkerStats protected by mutex.
struct WorkerStatsInner {
    max_task_label: String,
    thread_stats_map: HashMap<u64, crate::corpus::computations::ThreadStats>,
}

/// Thread-safe worker statistics, isolated from Witch struct.
///
/// This follows the proven pattern from DbThreadHandle - stats are stored in
/// a separate heap allocation via Arc, with atomic counters for numeric data
/// and a mutex for complex data (labels, thread_stats_map).
pub(super) struct SharedWorkerStats {
    tasks_completed: AtomicU64,
    total_task_duration_ms: AtomicU64,
    total_queue_wait_ms: AtomicU64,
    max_task_ms: AtomicU64,
    max_queue_wait_ms: AtomicU64,
    /// Complex data behind mutex
    inner: Mutex<WorkerStatsInner>,
}

impl SharedWorkerStats {
    pub(super) fn new() -> Self {
        Self {
            tasks_completed: AtomicU64::new(0),
            total_task_duration_ms: AtomicU64::new(0),
            total_queue_wait_ms: AtomicU64::new(0),
            max_task_ms: AtomicU64::new(0),
            max_queue_wait_ms: AtomicU64::new(0),
            inner: Mutex::new(WorkerStatsInner {
                max_task_label: String::new(),
                thread_stats_map: HashMap::new(),
            }),
        }
    }

    /// Atomically update max value if new value is larger.
    /// Returns true if max was updated.
    fn update_max(atomic: &AtomicU64, new_value: u64) -> bool {
        let mut current_max = atomic.load(Ordering::Relaxed);
        while new_value > current_max {
            match atomic.compare_exchange_weak(
                current_max,
                new_value,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(actual) => current_max = actual,
            }
        }
        false
    }

    /// Record a completed task result. Called from main thread's tick().
    pub(super) fn record_result(&self, result: &TaskResult) {
        self.tasks_completed.fetch_add(1, Ordering::Relaxed);
        self.total_task_duration_ms.fetch_add(result.duration_ms, Ordering::Relaxed);
        self.total_queue_wait_ms.fetch_add(result.queue_wait_ms, Ordering::Relaxed);

        // Atomic max update for task duration
        if Self::update_max(&self.max_task_ms, result.duration_ms) {
            // Update label under mutex if we set a new max
            if let Ok(mut inner) = self.inner.lock() {
                inner.max_task_label = result.label.clone();
            }
        }

        // Atomic max update for queue wait
        Self::update_max(&self.max_queue_wait_ms, result.queue_wait_ms);

        // Thread stats under mutex
        if let Some(ref thread_stats) = result.thread_stats {
            if let Ok(mut inner) = self.inner.lock() {
                inner.thread_stats_map.insert(thread_stats.thread_id, thread_stats.clone());
            }
        }
    }

    /// Get a snapshot of current stats for UI display.
    pub(super) fn snapshot(&self) -> WorkerStats {
        let tasks = self.tasks_completed.load(Ordering::Relaxed);
        let total_duration = self.total_task_duration_ms.load(Ordering::Relaxed);
        let total_queue = self.total_queue_wait_ms.load(Ordering::Relaxed);
        let max_task = self.max_task_ms.load(Ordering::Relaxed);
        let max_queue = self.max_queue_wait_ms.load(Ordering::Relaxed);

        let inner = self.inner.lock().unwrap();

        // Aggregate per-thread stats
        let active_threads = inner.thread_stats_map.len();
        let mut total_db_read_us = 0u64;
        let mut total_db_reads = 0u64;
        let mut max_db_read_us = 0u64;
        let mut all_samples: Vec<u64> = Vec::new();

        for stats in inner.thread_stats_map.values() {
            total_db_read_us += stats.total_db_read_us;
            total_db_reads += stats.db_read_count;
            if stats.max_db_read_us > max_db_read_us {
                max_db_read_us = stats.max_db_read_us;
            }
            // Collect samples for median calculation
            all_samples.extend_from_slice(&stats.read_samples[..stats.read_sample_count]);
        }

        let avg_db_read_us = if total_db_reads > 0 {
            total_db_read_us / total_db_reads
        } else {
            0
        };

        // Compute median from samples
        let median_db_read_us = if all_samples.is_empty() {
            0
        } else {
            all_samples.sort_unstable();
            all_samples[all_samples.len() / 2]
        };

        WorkerStats {
            tasks_completed: tasks,
            avg_task_ms: if tasks > 0 { total_duration / tasks } else { 0 },
            max_task_ms: max_task,
            max_task_label: inner.max_task_label.clone(),
            queue_wait_avg_ms: if tasks > 0 { total_queue / tasks } else { 0 },
            queue_wait_max_ms: max_queue,
            active_threads,
            avg_db_read_us,
            median_db_read_us,
            max_db_read_us,
            total_db_reads,
        }
    }

    /// Debug: get raw values for logging
    pub(super) fn debug_values(&self) -> (u64, u64, u64) {
        (
            self.tasks_completed.load(Ordering::Relaxed),
            self.total_queue_wait_ms.load(Ordering::Relaxed),
            self.max_queue_wait_ms.load(Ordering::Relaxed),
        )
    }
}
