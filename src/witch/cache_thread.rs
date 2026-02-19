//! Dedicated cache thread for UI read queries.
//!
//! The cache thread owns a read-only DB connection and handles both:
//! - **Periodic refreshes** (throttled, demand-driven): insights, inbox, deploy
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

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::config;
use crate::meta::views::{DeployStatus, InboxOverviewData, InsightsData};
use crate::db::{Database, ReadOnlyDb};

// ============================================================================
// Cache Thread Protocol
// ============================================================================

/// Requests from UI/Witch → cache thread.
pub(crate) enum CacheRequest {
    /// UI wants fresh insights data (throttled).
    WantInsights,
    /// UI wants fresh inbox overview data (throttled).
    WantInbox,
    /// UI wants fresh deploy status (throttled).
    WantDeploy,
    /// Invalidate all cached data (force re-query on next want).
    InvalidateAll,
    /// Execute a one-shot query on the read-only connection.
    Query(Box<dyn FnOnce(&ReadOnlyDb<'_>) + Send>),
    /// Close and reopen the read-only connection (after schema migrations).
    ReconnectDb,
    /// Shut down the cache thread.
    Shutdown,
}

/// Periodic refresh results from cache thread → UI.
pub(crate) enum CacheReady {
    Insights(InsightsData),
    InboxOverview(InboxOverviewData),
    DeployStatus(DeployStatus),
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
    /// Signal demand for insights data (throttled by cache thread).
    pub(crate) fn want_insights(&self) {
        let _ = self.request_tx.send(CacheRequest::WantInsights);
    }

    /// Signal demand for inbox overview data (throttled by cache thread).
    pub(crate) fn want_inbox(&self) {
        let _ = self.request_tx.send(CacheRequest::WantInbox);
    }

    /// Signal demand for deploy status (throttled by cache thread).
    pub(crate) fn want_deploy(&self) {
        let _ = self.request_tx.send(CacheRequest::WantDeploy);
    }

    /// Invalidate all cached data. Next want_* call will force a re-query.
    pub(crate) fn invalidate_all(&self) {
        let _ = self.request_tx.send(CacheRequest::InvalidateAll);
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
    pub(crate) fn query<T: Send + 'static>(
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
    /// Tell the cache thread to invalidate all cached data.
    pub(crate) fn invalidate_all(&self) {
        let _ = self.request_tx.send(CacheRequest::InvalidateAll);
    }

    /// Shut down the cache thread and wait for it to exit.
    pub(crate) fn shutdown(&mut self) {
        let _ = self.request_tx.send(CacheRequest::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

// ============================================================================
// Cache Thread Spawn
// ============================================================================

/// Throttle state for periodic cache entries.
struct ThrottleState {
    insights_wanted: bool,
    inbox_wanted: bool,
    deploy_wanted: bool,
    insights_at: Option<Instant>,
    inbox_at: Option<Instant>,
    deploy_at: Option<Instant>,
}

impl ThrottleState {
    const INSIGHTS_THROTTLE: Duration = Duration::from_secs(30);
    const INBOX_THROTTLE: Duration = Duration::from_secs(15);
    const DEPLOY_THROTTLE: Duration = Duration::from_secs(15);

    fn new() -> Self {
        Self {
            insights_wanted: false,
            inbox_wanted: false,
            deploy_wanted: false,
            insights_at: None,
            inbox_at: None,
            deploy_at: None,
        }
    }

    fn invalidate_all(&mut self) {
        self.insights_at = None;
        self.inbox_at = None;
        self.deploy_at = None;
        // Set wanted so next cycle refreshes everything
        self.insights_wanted = true;
        self.inbox_wanted = true;
        self.deploy_wanted = true;
    }

    fn should_refresh_insights(&self) -> bool {
        self.insights_wanted && self.insights_at
            .map_or(true, |t| t.elapsed() >= Self::INSIGHTS_THROTTLE)
    }

    fn should_refresh_inbox(&self) -> bool {
        self.inbox_wanted && self.inbox_at
            .map_or(true, |t| t.elapsed() >= Self::INBOX_THROTTLE)
    }

    fn should_refresh_deploy(&self) -> bool {
        self.deploy_wanted && self.deploy_at
            .map_or(true, |t| t.elapsed() >= Self::DEPLOY_THROTTLE)
    }
}

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
fn cache_thread_main(
    request_rx: Receiver<CacheRequest>,
    ready_tx: Sender<CacheReady>,
) {
    crate::logging::log_general("[CACHE_THREAD] Started");

    let mut db = open_read_only_db();
    let mut throttle = ThrottleState::new();

    loop {
        // Block with timeout — gives us a natural refresh cycle
        match request_rx.recv_timeout(Duration::from_millis(250)) {
            Ok(request) => {
                // Process this request and drain any queued ones
                if process_request(request, &mut db, &mut throttle, &ready_tx) {
                    break; // Shutdown requested
                }
                // Drain queued requests
                while let Ok(request) = request_rx.try_recv() {
                    if process_request(request, &mut db, &mut throttle, &ready_tx) {
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
            run_refreshes(&mut throttle, &read_db, &ready_tx);
        }
    }

    crate::logging::log_general("[CACHE_THREAD] Shutdown complete");
}

/// Process a single cache request. Returns true if shutdown was requested.
fn process_request(
    request: CacheRequest,
    db: &mut Option<Database>,
    throttle: &mut ThrottleState,
    ready_tx: &Sender<CacheReady>,
) -> bool {
    match request {
        CacheRequest::WantInsights => {
            throttle.insights_wanted = true;
        }
        CacheRequest::WantInbox => {
            throttle.inbox_wanted = true;
        }
        CacheRequest::WantDeploy => {
            throttle.deploy_wanted = true;
        }
        CacheRequest::InvalidateAll => {
            throttle.invalidate_all();
            // Run an immediate refresh cycle
            if let Some(ref db_conn) = db {
                let read_db = ReadOnlyDb::new(db_conn);
                run_refreshes(throttle, &read_db, ready_tx);
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
                    "[CACHE_THREAD] Query received but no DB connection available"
                );
            }
        }
        CacheRequest::ReconnectDb => {
            crate::logging::log_general("[CACHE_THREAD] Reconnecting DB");
            *db = open_read_only_db();
        }
        CacheRequest::Shutdown => {
            return true;
        }
    }
    false
}

/// Run throttled periodic refreshes.
fn run_refreshes(
    throttle: &mut ThrottleState,
    read_db: &ReadOnlyDb<'_>,
    ready_tx: &Sender<CacheReady>,
) {
    if throttle.should_refresh_insights() {
        throttle.insights_wanted = false;
        if let Ok(data) = read_db.get_insights_data() {
            throttle.insights_at = Some(Instant::now());
            let _ = ready_tx.send(CacheReady::Insights(data));
        }
    }

    if throttle.should_refresh_inbox() {
        throttle.inbox_wanted = false;
        if let Ok(data) = read_db.get_inbox_overview_data() {
            throttle.inbox_at = Some(Instant::now());
            let _ = ready_tx.send(CacheReady::InboxOverview(data));
        }
    }

    if throttle.should_refresh_deploy() {
        throttle.deploy_wanted = false;
        if let Ok(data) = read_db.get_deploy_status() {
            throttle.deploy_at = Some(Instant::now());
            let _ = ready_tx.send(CacheReady::DeployStatus(data));
        }
    }
}
