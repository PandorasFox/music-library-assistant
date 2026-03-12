//! Dedicated cache thread for UI read queries.
//!
//! The cache thread owns a read-only DB connection and handles both:
//! - **Periodic refreshes** (throttled, demand-driven, scope-invalidated)
//! - **One-shot queries** (typed, async): modal init, view data loading
//!
//! ## Architecture
//!
//! The UI sends `CacheRequest` variants via channel. The cache thread loop
//! processes them with `recv_timeout(250ms)`, running throttled refreshes when
//! demand flags are set and executing one-shot closures immediately.
//!
//! Results flow back via two paths:
//! - `CacheReady` channel for periodic data (UI drains each frame)
//! - Per-query `Sender<T>` channels for one-shot queries (typed `DbQuery<T>`)
//!
//! ## Adding a Cached Query
//!
//! 1. Define it in `db/domain.rs` with `cached(N, SCOPE)` syntax
//! 2. Call `app.cache.want::<GetFoo>()` in the event loop
//! 3. Read with `app.cached.get::<GetFoo>()`
//!
//! That's it. The cache thread auto-registers slots on first `want`.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config;
use crate::db::domain::{CachedQuery, DomainQuery};
use crate::db::{Database, ReadOnlyDb};
use crate::meta::recomputation::RecomputationScope;

// ============================================================================
// Cache Thread Protocol
// ============================================================================

/// Factory closure that builds a `RegisteredSlot` from trait constants.
/// Sent with every `Want` request; `entry().or_insert_with()` deduplicates.
type CacheSlotFactory = Box<dyn FnOnce() -> RegisteredSlot + Send>;

/// Requests from UI/Witch → cache thread.
pub(crate) enum CacheRequest {
    /// Signal demand for a cached query. Factory is used to register the slot
    /// on first request; subsequent requests just set the `wanted` flag.
    Want {
        type_id: TypeId,
        factory: CacheSlotFactory,
        urgent: bool,
    },
    /// Invalidate cached entries whose scope overlaps with the given scope.
    InvalidateScope(RecomputationScope),
    /// Execute a one-shot query on the read-only connection.
    Query(Box<dyn FnOnce(&ReadOnlyDb<'_>) + Send>),
    /// Close and reopen the read-only connection (after schema migrations).
    ReconnectDb,
    /// Update SQLite PRAGMA cache_size on the cache thread's connection.
    SetCacheSize(i64),
    /// Shut down the cache thread.
    Shutdown,
}

/// A single periodic refresh result from cache thread → UI.
pub(crate) struct CacheReady {
    pub type_id: TypeId,
    pub value: Box<dyn Any + Send>,
}

// ============================================================================
// GenericCache (internal slot management)
// ============================================================================

/// A registered cache slot with its execution closure and throttle state.
pub(crate) struct RegisteredSlot {
    /// Closure that executes the query and returns a boxed result.
    execute: Box<dyn Fn(&ReadOnlyDb<'_>) -> Box<dyn Any + Send> + Send>,
    /// Normal throttle interval.
    throttle: Duration,
    /// Optional shorter throttle for urgent refresh.
    urgent_throttle: Option<Duration>,
    /// Which mutation domains affect this query.
    scope: RecomputationScope,
    /// Whether the UI has signaled demand since last refresh.
    wanted: bool,
    /// Whether urgent throttle should be used.
    urgent: bool,
    /// When this slot was last refreshed (None = never).
    refreshed_at: Option<Instant>,
}

impl RegisteredSlot {
    fn should_refresh(&self) -> bool {
        if !self.wanted {
            return false;
        }
        let throttle = if self.urgent {
            self.urgent_throttle.unwrap_or(self.throttle)
        } else {
            self.throttle
        };
        self.refreshed_at
            .is_none_or(|t| t.elapsed() >= throttle)
    }
}

/// Internal cache state: TypeId-keyed slots for all registered cached queries.
struct GenericCache {
    slots: HashMap<TypeId, RegisteredSlot>,
}

impl GenericCache {
    fn new() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }

    /// Invalidate all slots whose scope overlaps with the given mutation scope.
    fn invalidate_scope(&mut self, scope: RecomputationScope) {
        for slot in self.slots.values_mut() {
            if slot.scope.overlaps(scope) {
                slot.refreshed_at = None;
                slot.wanted = true;
            }
        }
    }

    /// Run throttled refreshes for all slots that are due.
    fn run_refreshes(&mut self, read_db: &ReadOnlyDb<'_>, ready_tx: &Sender<CacheReady>) {
        for (&type_id, slot) in &mut self.slots {
            if slot.should_refresh() {
                slot.wanted = false;
                let value = (slot.execute)(read_db);
                slot.refreshed_at = Some(Instant::now());
                let _ = ready_tx.send(CacheReady { type_id, value });
            }
        }
    }
}

// ============================================================================
// CacheHandle (given to UI)
// ============================================================================

/// Handle for UI code to interact with the cache thread.
///
/// Cheap to clone (just channel senders/receivers are Arc-wrapped internally).
pub(crate) struct CacheHandle {
    request_tx: Sender<CacheRequest>,
    ready_rx: Receiver<CacheReady>,
}

impl CacheHandle {
    /// Signal demand for a cached query (normal throttle).
    pub(crate) fn want<Q: CachedQuery>(&self) {
        self.send_want::<Q>(false);
    }

    /// Signal urgent demand for a cached query (uses urgent throttle if defined).
    pub(crate) fn want_urgent<Q: CachedQuery>(&self) {
        self.send_want::<Q>(true);
    }

    fn send_want<Q: CachedQuery>(&self, urgent: bool) {
        let type_id = TypeId::of::<Q>();
        let factory: CacheSlotFactory = Box::new(move || RegisteredSlot {
            execute: Box::new(|db| Box::new(Q::default().execute(db))),
            throttle: Q::THROTTLE,
            urgent_throttle: Q::URGENT_THROTTLE,
            scope: Q::SCOPE,
            wanted: true,
            urgent,
            refreshed_at: None,
        });
        let _ = self.request_tx.send(CacheRequest::Want {
            type_id,
            factory,
            urgent,
        });
    }

    /// Invalidate cached entries whose scope overlaps with the given scope.
    pub(crate) fn invalidate_scope(&self, scope: RecomputationScope) {
        let _ = self.request_tx.send(CacheRequest::InvalidateScope(scope));
    }

    /// Tell the cache thread to close and reopen its DB connection.
    /// Used after schema migrations.
    pub(crate) fn reconnect_db(&self) {
        let _ = self.request_tx.send(CacheRequest::ReconnectDb);
    }

    /// Drain all ready results from the cache thread.
    ///
    /// Non-blocking. Returns whatever is available right now.
    pub(crate) fn drain_ready(&self) -> Vec<CacheReady> {
        let mut results = Vec::new();
        while let Ok(item) = self.ready_rx.try_recv() {
            results.push(item);
        }
        results
    }

    /// Submit a one-shot query to run on the cache thread's DB connection.
    ///
    /// Returns a `DbQuery<T>` that can be polled or blocked on for the result.
    /// The closure receives a `&ReadOnlyDb` and should return the data via
    /// the captured `Sender<T>`.
    fn query<T: Send + 'static>(
        &self,
        f: impl FnOnce(&ReadOnlyDb<'_>) -> T + Send + 'static,
    ) -> DbQuery<T> {
        let (tx, rx) = mpsc::channel();
        let boxed: Box<dyn FnOnce(&ReadOnlyDb<'_>) + Send> = Box::new(move |db| {
            let result = f(db);
            let _ = tx.send(result);
        });
        let _ = self.request_tx.send(CacheRequest::Query(boxed));
        DbQuery { rx }
    }

    /// Submit a typed domain query to run on the cache thread's DB connection.
    ///
    /// All UI→DB reads go through this method. The query runs on the cache
    /// thread and the result arrives via the returned `DbQuery` handle.
    pub(crate) fn domain_query<Q: DomainQuery>(&self, q: Q) -> DbQuery<Q::Response> {
        self.query(move |db| q.execute(db))
    }
}

// ============================================================================
// DbQuery<T> (typed one-shot response)
// ============================================================================

/// A pending one-shot query result.
///
/// Wraps a channel receiver. The query runs on the cache thread;
/// the result arrives when complete.
pub(crate) struct DbQuery<T> {
    rx: Receiver<T>,
}

impl<T> DbQuery<T> {
    /// Blocking wait for the result.
    pub(crate) fn recv(self) -> T {
        self.rx.recv().expect("cache thread dropped query sender")
    }

    /// Non-blocking poll for the result.
    ///
    /// Returns `Ok(value)` if ready, `Err(self)` if still pending (returns self
    /// back so you can try again next frame).
    pub(crate) fn try_recv(self) -> Result<T, Self> {
        match self.rx.try_recv() {
            Ok(value) => Ok(value),
            Err(std::sync::mpsc::TryRecvError::Empty) => Err(self),
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                panic!("cache thread dropped query sender")
            }
        }
    }
}

// ============================================================================
// CacheThreadHandle (kept by Witch)
// ============================================================================

/// Handle for the Witch to manage the cache thread's lifecycle.
pub(crate) struct CacheThreadHandle {
    join_handle: Option<JoinHandle<()>>,
    request_tx: Sender<CacheRequest>,
}

impl CacheThreadHandle {
    /// Tell the cache thread to invalidate entries whose scope overlaps.
    pub(crate) fn invalidate_scope(&self, scope: RecomputationScope) {
        let _ = self.request_tx.send(CacheRequest::InvalidateScope(scope));
    }

    /// Tell the cache thread to close and reopen its DB connection.
    /// Used after first-time setup creates the DB.
    pub(crate) fn reconnect_db(&self) {
        let _ = self.request_tx.send(CacheRequest::ReconnectDb);
    }

    /// Update SQLite PRAGMA cache_size on the cache thread's connection.
    pub(crate) fn set_cache_size(&self, kb: i64) {
        let _ = self.request_tx.send(CacheRequest::SetCacheSize(kb));
    }
}

impl super::types::ManagedThread for CacheThreadHandle {
    fn send_shutdown(&self) {
        let _ = self.request_tx.send(CacheRequest::Shutdown);
    }

    fn take_handle(&mut self) -> Option<std::thread::JoinHandle<()>> {
        self.join_handle.take()
    }
}

impl Drop for CacheThreadHandle {
    fn drop(&mut self) {
        use super::types::ManagedThread;
        self.shutdown();
    }
}

// ============================================================================
// Cache Thread Spawn
// ============================================================================

/// Spawn the cache thread. Returns the UI handle and the Witch handle.
pub(crate) fn spawn() -> (CacheHandle, CacheThreadHandle) {
    let (request_tx, request_rx) = mpsc::channel();
    let (ready_tx, ready_rx) = mpsc::channel();

    let witch_request_tx = request_tx.clone();

    let join_handle = std::thread::Builder::new()
        .name("mm-cache".to_string())
        .spawn(move || {
            cache_thread_main(request_rx, ready_tx);
        })
        .expect("failed to spawn cache thread");

    let ui_handle = CacheHandle {
        request_tx,
        ready_rx,
    };

    let witch_handle = CacheThreadHandle {
        join_handle: Some(join_handle),
        request_tx: witch_request_tx,
    };

    (ui_handle, witch_handle)
}

/// Open (or reopen) the read-only database connection for the cache thread.
fn open_read_only_db() -> Option<Database> {
    let db_path = config::get_db_path().ok()?;
    Database::open_read_only(&db_path).ok()
}

/// Main loop for the cache thread.
fn cache_thread_main(request_rx: Receiver<CacheRequest>, ready_tx: Sender<CacheReady>) {
    crate::logging::log_general("[CACHE_THREAD] Started");

    let mut db = open_read_only_db();
    let mut cache = GenericCache::new();

    loop {
        // Block with timeout — gives us a natural refresh cycle
        match request_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(request) => {
                // Process this request and drain any queued ones
                if process_request(request, &mut db, &mut cache, &ready_tx) {
                    break; // Shutdown requested
                }
                // Drain queued requests
                while let Ok(request) = request_rx.try_recv() {
                    if process_request(request, &mut db, &mut cache, &ready_tx) {
                        crate::logging::log_general("[CACHE_THREAD] Shutdown complete");
                        return;
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Normal timeout — proceed to refresh cycle
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                crate::logging::log_general("[CACHE_THREAD] Channel disconnected, shutting down");
                break;
            }
        }

        // Run throttled refreshes if DB is available
        if let Some(ref db_conn) = db {
            let read_db = ReadOnlyDb::new(db_conn);
            cache.run_refreshes(&read_db, &ready_tx);
        }
    }

    crate::logging::log_general("[CACHE_THREAD] Shutdown complete");
}

/// Process a single cache request. Returns true if shutdown was requested.
fn process_request(
    request: CacheRequest,
    db: &mut Option<Database>,
    cache: &mut GenericCache,
    ready_tx: &Sender<CacheReady>,
) -> bool {
    match request {
        CacheRequest::Want {
            type_id,
            factory,
            urgent,
        } => {
            let slot = cache.slots.entry(type_id).or_insert_with(factory);
            slot.wanted = true;
            if urgent {
                slot.urgent = true;
            }
        }
        CacheRequest::InvalidateScope(scope) => {
            cache.invalidate_scope(scope);
            // Run an immediate refresh cycle
            if let Some(ref db_conn) = db {
                let read_db = ReadOnlyDb::new(db_conn);
                cache.run_refreshes(&read_db, ready_tx);
            }
        }
        CacheRequest::Query(f) => {
            if let Some(ref db_conn) = db {
                let read_db = ReadOnlyDb::new(db_conn);
                f(&read_db);
            } else {
                // DB not available — closure's sender will be dropped,
                // causing recv() on DbQuery to panic. This should only happen
                // during very early startup before DB exists.
                crate::logging::log_error(
                    "[CACHE_THREAD] Query received but no DB connection available",
                );
            }
        }
        CacheRequest::ReconnectDb => {
            crate::logging::log_general("[CACHE_THREAD] Reconnecting DB");
            *db = open_read_only_db();
        }
        CacheRequest::SetCacheSize(kb) => {
            if let Some(ref db_conn) = db {
                let _ = db_conn
                    .conn()
                    .execute_batch(&format!("PRAGMA cache_size = {};", kb));
                crate::logging::log_general(format!(
                    "[CACHE_THREAD] Updated cache_size to {} KB",
                    kb
                ));
            }
        }
        CacheRequest::Shutdown => {
            return true;
        }
    }
    false
}
