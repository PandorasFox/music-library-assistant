//! Background cache for UI read queries.
//!
//! This module provides demand-driven, throttled caching for expensive DB queries
//! that the UI needs. Instead of blocking the UI thread with synchronous queries,
//! UI components flag "I want this data" and the Witch spawns background refreshes
//! on the rayon pool.
//!
//! ## Usage Pattern
//!
//! ```ignore
//! // UI code (every frame when data is needed):
//! witch.ui_read_cache().want_insights_data();
//!
//! // Render code (reads latest cached value, never blocks):
//! let data = witch.ui_read_cache().insights_data();
//! ```
//!
//! ## Design
//!
//! - **Demand-driven**: No refresh unless UI explicitly wants the data
//! - **Throttled**: Configurable minimum interval between refreshes (default 15s)
//! - **Non-blocking**: UI never waits; reads whatever is cached (may be stale)
//! - **Generic**: `CacheEntry<T>` template makes adding new cached values mechanical

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use crate::config;
use crate::corpus::db::types::InsightsData;
use crate::corpus::db::Database;

// ============================================================================
// Shutdown Coordination
// ============================================================================

/// Global shutdown flag for UI cache refreshes.
///
/// When set to true, no new refresh tasks will be spawned. This is set by
/// `shutdown_ui_cache()` before closing DB connections.
static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Count of in-flight refresh tasks.
///
/// Incremented when a refresh task starts, decremented when it completes.
/// Used during shutdown to wait for all refreshes to finish.
static IN_FLIGHT_REFRESHES: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Request shutdown of UI cache refreshes and wait for in-flight tasks.
///
/// Called by `Witch::drop()` before closing DB connections. This:
/// 1. Sets the shutdown flag to prevent new refreshes
/// 2. Waits for any in-flight refresh tasks to complete
///
/// After this returns, no UI cache refresh tasks will have open DB connections.
pub fn shutdown_ui_cache() {
    // Signal no new refreshes should start
    SHUTDOWN_REQUESTED.store(true, Ordering::Release);

    // Wait for in-flight refreshes to complete (with timeout)
    let start = Instant::now();
    let timeout = Duration::from_secs(5);

    while IN_FLIGHT_REFRESHES.load(Ordering::Acquire) > 0 {
        if start.elapsed() > timeout {
            crate::logging::log_error(format!(
                "[UI_CACHE] Shutdown timeout: {} refreshes still in-flight after {:?}",
                IN_FLIGHT_REFRESHES.load(Ordering::Acquire),
                timeout
            ));
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    crate::logging::log_general("[UI_CACHE] Shutdown complete, all refresh tasks finished");
}

/// Check if shutdown has been requested for UI cache refreshes.
fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::Acquire)
}

// ============================================================================
// CacheEntry<T> - Generic cache slot
// ============================================================================

/// A single cached value with demand-driven background refresh.
///
/// Thread-safe: UI thread reads, worker threads write.
pub struct CacheEntry<T: Clone + Send + Sync + 'static> {
    shared: Arc<CacheShared<T>>,
    throttle: Duration,
}

struct CacheShared<T> {
    /// UI has flagged that it wants this data refreshed.
    wanted: AtomicBool,
    /// A refresh task is currently in-flight.
    refreshing: AtomicBool,
    /// The cached data and timestamp.
    inner: RwLock<CacheInner<T>>,
}

struct CacheInner<T> {
    /// The cached data, if ever computed.
    data: Option<T>,
    /// When the data was last refreshed. None if never refreshed.
    /// Used for throttle checks in want().
    updated_at: Option<Instant>,
}

impl<T: Clone + Send + Sync + 'static> CacheEntry<T> {
    /// Create a new cache entry with the given throttle duration.
    ///
    /// The throttle prevents refresh spam - want() calls are ignored if
    /// a refresh completed less than `throttle` ago.
    pub fn new(throttle: Duration) -> Self {
        Self {
            shared: Arc::new(CacheShared {
                wanted: AtomicBool::new(false),
                refreshing: AtomicBool::new(false),
                inner: RwLock::new(CacheInner {
                    data: None,
                    updated_at: None,
                }),
            }),
            throttle,
        }
    }

    /// UI calls this to express demand for this data.
    ///
    /// Idempotent and cheap. Sets the `wanted` flag if:
    /// 1. Not already refreshing (in-flight check)
    /// 2. Either never refreshed, or last refresh was >= throttle ago
    ///
    /// The Witch's tick() will notice the flag and spawn a refresh task.
    pub fn want(&self) {
        // Fast path: already refreshing, no need to check timestamp
        if self.shared.refreshing.load(Ordering::Relaxed) {
            return;
        }

        // Check throttle (requires lock, but only briefly)
        if let Ok(inner) = self.shared.inner.read() {
            if let Some(updated_at) = inner.updated_at {
                if updated_at.elapsed() < self.throttle {
                    return; // Too fresh, ignore
                }
            }
        }

        // Set wanted flag for daemon to pick up
        self.shared.wanted.store(true, Ordering::Relaxed);
    }

    /// Invalidate the cache, bypassing the throttle.
    ///
    /// Clears `updated_at` so the next `want()` call will trigger a refresh
    /// regardless of how recently the data was updated. Use this after
    /// mutations complete to ensure fresh data on next view.
    pub fn invalidate(&self) {
        if let Ok(mut inner) = self.shared.inner.write() {
            inner.updated_at = None;
        }
        // Also set wanted so refresh happens immediately
        self.shared.wanted.store(true, Ordering::Relaxed);
    }

    /// Read the latest cached value. Never blocks (briefly for RwLock).
    ///
    /// Returns None if the data has never been computed.
    pub fn get(&self) -> Option<T> {
        self.shared
            .inner
            .read()
            .ok()
            .and_then(|inner| inner.data.clone())
    }

    /// The Witch calls this to check if a refresh is needed.
    ///
    /// Returns a `CacheWriter` handle if:
    /// - wanted flag is set
    /// - not already refreshing
    ///
    /// The caller should spawn a worker task that computes the value
    /// and calls `writer.complete(data)` or `writer.abort()`.
    pub(crate) fn take_refresh(&self) -> Option<CacheWriter<T>> {
        // Atomically clear wanted and check if we should proceed
        if !self.shared.wanted.swap(false, Ordering::Relaxed) {
            return None; // Not wanted
        }

        // Try to claim the refreshing slot
        if self
            .shared
            .refreshing
            .compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed)
            .is_err()
        {
            return None; // Already refreshing
        }

        // Return writer handle
        Some(CacheWriter {
            shared: Arc::clone(&self.shared),
        })
    }
}

// ============================================================================
// CacheWriter<T> - Worker completion handle
// ============================================================================

/// Handle for workers to complete or abort a cache refresh.
///
/// Must be consumed by calling either `complete()` or `abort()`.
/// Dropping without calling either will clear the refreshing flag
/// (equivalent to abort).
pub struct CacheWriter<T> {
    shared: Arc<CacheShared<T>>,
}

impl<T> CacheWriter<T> {
    /// Complete the refresh with computed data.
    ///
    /// - Sets data to Some(value)
    /// - Sets updated_at to now
    /// - Clears refreshing flag
    pub fn complete(self, data: T) {
        if let Ok(mut inner) = self.shared.inner.write() {
            inner.data = Some(data);
            inner.updated_at = Some(Instant::now());
        }
        self.shared.refreshing.store(false, Ordering::Relaxed);
    }

    /// Abort the refresh (e.g., on error).
    ///
    /// - Does NOT update data or timestamp
    /// - Clears refreshing flag (allowing retry on next want)
    pub fn abort(self) {
        self.shared.refreshing.store(false, Ordering::Relaxed);
    }
}

impl<T> Drop for CacheWriter<T> {
    fn drop(&mut self) {
        // Safety net: if dropped without complete/abort, clear refreshing
        // This prevents permanent "stuck refreshing" state on panic
        self.shared.refreshing.store(false, Ordering::Relaxed);
    }
}

// ============================================================================
// UiReadCache - Container for all cached UI data
// ============================================================================

/// Background cache for UI read queries.
///
/// Owned by the Witch. UI components call `want_*()` methods to flag demand,
/// and `*()` methods to read cached values.
pub struct UiReadCache {
    insights_data: CacheEntry<InsightsData>,
}

impl UiReadCache {
    /// Insights data throttle (30 seconds - heavier computation).
    const INSIGHTS_THROTTLE: Duration = Duration::from_secs(30);

    /// Create a new UI read cache with default throttle settings.
    pub fn new() -> Self {
        Self {
            insights_data: CacheEntry::new(Self::INSIGHTS_THROTTLE),
        }
    }

    // -------------------------------------------------------------------------
    // Insights Data
    // -------------------------------------------------------------------------

    /// UI calls this when it wants insights data.
    ///
    /// Idempotent - safe to call every frame. Respects throttle.
    pub fn want_insights_data(&self) {
        self.insights_data.want();
    }

    /// Read the latest cached insights data.
    ///
    /// Returns None if never computed. Never blocks.
    pub fn insights_data(&self) -> Option<InsightsData> {
        self.insights_data.get()
    }

    /// Invalidate insights data cache, forcing refresh on next want().
    ///
    /// Call this after mutations complete to ensure fresh data when
    /// transitioning to insights view.
    pub fn invalidate_insights_data(&self) {
        self.insights_data.invalidate();
    }

    // -------------------------------------------------------------------------
    // Witch Integration
    // -------------------------------------------------------------------------

    /// The Witch calls this in tick() to spawn refresh tasks for flagged entries.
    ///
    /// Spawns rayon tasks for any entries that need refreshing.
    /// Respects shutdown flag - no new tasks are spawned after shutdown is requested.
    pub(crate) fn spawn_refreshes(&self) {
        // Don't spawn new refreshes if shutdown is in progress
        if is_shutdown_requested() {
            return;
        }

        // Insights data refresh
        if let Some(writer) = self.insights_data.take_refresh() {
            IN_FLIGHT_REFRESHES.fetch_add(1, Ordering::Release);
            rayon::spawn(move || {
                // Open fresh read-only connection on worker thread
                if let Ok(db_path) = config::get_db_path() {
                    if let Ok(db) = Database::open_read_only(&db_path) {
                        if let Ok(data) = db.get_insights_data() {
                            writer.complete(data);
                            IN_FLIGHT_REFRESHES.fetch_sub(1, Ordering::Release);
                            return;
                        }
                    }
                }
                // On error, abort (allows retry on next want)
                writer.abort();
                IN_FLIGHT_REFRESHES.fetch_sub(1, Ordering::Release);
            });
        }
    }
}

impl Default for UiReadCache {
    fn default() -> Self {
        Self::new()
    }
}
