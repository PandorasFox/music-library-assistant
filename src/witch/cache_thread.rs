//! DB read thread — executes domain queries on behalf of protocol clients.
//!
//! Owns a read-only DB connection. Receives `(DomainQueryPayload, reply_tx)`
//! pairs forwarded by the Witch after auth validation. Executes the query
//! via `dispatch_domain_query` and sends the result directly back to the
//! client through the forwarded reply channel — the Witch never touches
//! the response path for domain queries.

use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::JoinHandle;

use crate::config;
use crate::db::domain::{dispatch_domain_query, DomainQueryPayload};
use crate::db::{Database, ReadOnlyDb};
use crate::meta::recomputation::RecomputationScope;

// ============================================================================
// Read Thread Protocol
// ============================================================================

/// The reply type that clients expect from authenticated protocol requests.
pub(super) type AuthReply = Sender<Result<
    crate::meta::protocol::AuthenticatedResponse,
    crate::meta::protocol::ProtocolError,
>>;

/// Requests from Witch → read thread.
pub(super) enum CacheRequest {
    /// Domain query with the client's original reply channel.
    /// The read thread executes the query, wraps in AuthenticatedResponse, and
    /// replies directly — the Witch never touches the response path.
    DomainQuery {
        payload: Box<DomainQueryPayload>,
        reply: AuthReply,
    },
    /// Invalidate cached results whose scope overlaps with the given scope.
    InvalidateScope(RecomputationScope),
    /// Close and reopen the read-only connection (after schema migrations).
    ReconnectDb,
    /// Update SQLite PRAGMA cache_size on the read thread's connection.
    SetCacheSize(i64),
    /// Shut down the read thread.
    Shutdown,
}

// ============================================================================
// CacheThreadHandle (kept by Witch)
// ============================================================================

/// Handle for the Witch to manage the read thread's lifecycle and forward queries.
pub(crate) struct CacheThreadHandle {
    join_handle: Option<JoinHandle<()>>,
    request_tx: Sender<CacheRequest>,
}

impl CacheThreadHandle {
    /// Forward a domain query to the read thread with the client's reply channel.
    ///
    /// The read thread executes the query and replies directly to the client.
    /// The Witch does not wait for or touch the response.
    pub(super) fn forward_domain_query(
        &self,
        payload: DomainQueryPayload,
        reply: AuthReply,
    ) {
        let _ = self
            .request_tx
            .send(CacheRequest::DomainQuery { payload: Box::new(payload), reply });
    }

    /// Tell the read thread to invalidate cached results whose scope overlaps.
    pub(super) fn invalidate_scope(&self, scope: RecomputationScope) {
        let _ = self.request_tx.send(CacheRequest::InvalidateScope(scope));
    }

    /// Tell the read thread to close and reopen its DB connection.
    /// Used after first-time setup creates the DB.
    pub(super) fn reconnect_db(&self) {
        let _ = self.request_tx.send(CacheRequest::ReconnectDb);
    }

    /// Update SQLite PRAGMA cache_size on the read thread's connection.
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
// Read Thread Spawn
// ============================================================================

/// Spawn the read thread. Returns the Witch-side handle.
pub(super) fn spawn() -> CacheThreadHandle {
    let (request_tx, request_rx) = mpsc::channel();

    let join_handle = std::thread::Builder::new()
        .name("mm-read".to_string())
        .spawn(move || {
            read_thread_main(request_rx);
        })
        .expect("failed to spawn read thread");

    CacheThreadHandle {
        join_handle: Some(join_handle),
        request_tx,
    }
}

/// Open (or reopen) the read-only database connection for the read thread.
fn open_read_only_db() -> Option<Database> {
    let db_path = config::get_db_path().ok()?;
    Database::open_read_only(&db_path).ok()
}

/// Main loop for the read thread.
fn read_thread_main(request_rx: Receiver<CacheRequest>) {
    crate::logging::log_general("[READ_THREAD] Started");

    let mut db = open_read_only_db();

    loop {
        match request_rx.recv() {
            Ok(request) => {
                if process_request(request, &mut db) {
                    break;
                }
                // Drain queued requests
                while let Ok(request) = request_rx.try_recv() {
                    if process_request(request, &mut db) {
                        crate::logging::log_general("[READ_THREAD] Shutdown complete");
                        return;
                    }
                }
            }
            Err(mpsc::RecvError) => {
                crate::logging::log_general(
                    "[READ_THREAD] Channel disconnected, shutting down",
                );
                break;
            }
        }
    }

    crate::logging::log_general("[READ_THREAD] Shutdown complete");
}

/// Process a single request. Returns true if shutdown was requested.
fn process_request(
    request: CacheRequest,
    db: &mut Option<Database>,
) -> bool {
    match request {
        CacheRequest::DomainQuery { payload, reply } => {
            if let Some(ref db_conn) = db {
                let read_db = ReadOnlyDb::new(db_conn);
                let result = dispatch_domain_query(*payload, &read_db);
                let response = crate::meta::protocol::AuthenticatedResponse::Query(
                    Box::new(crate::meta::protocol::QueryResponse::Domain(result)),
                );
                let _ = reply.send(Ok(response));
            } else {
                let _ = reply.send(Err(crate::meta::protocol::ProtocolError::Internal(
                    "DB not available".to_string(),
                )));
            }
        }
        CacheRequest::InvalidateScope(_scope) => {
            // Scope-based invalidation reserved for future result caching.
            // Currently a no-op — every query hits the DB fresh.
        }
        CacheRequest::ReconnectDb => {
            crate::logging::log_general("[READ_THREAD] Reconnecting DB");
            *db = open_read_only_db();
        }
        CacheRequest::SetCacheSize(kb) => {
            if let Some(ref db_conn) = db {
                let _ = db_conn
                    .conn()
                    .execute_batch(&format!("PRAGMA cache_size = {};", kb));
                crate::logging::log_general(format!(
                    "[READ_THREAD] Updated cache_size to {} KB",
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
