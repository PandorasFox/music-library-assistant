//! Domain query dispatcher — spawns a thread per query with its own DB connection.
//!
//! Each domain query gets its own `spawn_blocking` task that opens a fresh
//! read-only SQLite connection, executes the query, replies directly to the
//! client, and exits. No shared state, no queue, no head-of-line blocking.
//!
//! SQLite WAL mode supports unlimited concurrent readers — each thread's
//! connection is independent.

use std::sync::Arc;
use std::sync::RwLock;

use tokio::sync::oneshot;

use crate::config;
use crate::db::domain::{dispatch_domain_query, DomainQueryPayload};
use crate::db::{Database, ReadOnlyDb};

// ============================================================================
// Types
// ============================================================================

/// The reply type that clients expect from authenticated protocol requests.
pub(super) type AuthReply = oneshot::Sender<Result<
    crate::meta::protocol::AuthenticatedResponse,
    crate::meta::protocol::ProtocolError,
>>;

/// Shared config handle (same Arc<RwLock<Config>> the Witch holds).
type SharedConfigInner = Arc<RwLock<config::Config>>;

// ============================================================================
// CacheThreadHandle (kept by Witch)
// ============================================================================

/// Handle for the Witch to dispatch domain queries.
///
/// Each query spawns its own blocking task with a fresh DB connection.
/// No persistent background thread — just fire-and-forget spawns.
pub(crate) struct CacheThreadHandle {
    shared_config: Option<SharedConfigInner>,
    cache_size_kb: Option<i64>,
}

impl CacheThreadHandle {
    /// Spawn a blocking task for this query. Opens its own DB connection,
    /// executes, replies, exits. No queue, no blocking other queries.
    pub(super) fn forward_domain_query(
        &self,
        payload: DomainQueryPayload,
        reply: AuthReply,
    ) {
        let shared_config = self.shared_config.clone();
        let cache_size_kb = self.cache_size_kb;

        tokio::task::spawn_blocking(move || {
            let db = match open_read_only_db(cache_size_kb) {
                Some(db) => db,
                None => {
                    let _ = reply.send(Err(crate::meta::protocol::ProtocolError::Internal(
                        "DB not available".to_string(),
                    )));
                    return;
                }
            };

            let read_db = ReadOnlyDb::new(&db);
            let config_guard = shared_config
                .as_ref()
                .map(|sc| sc.read().expect("config lock"));
            let config_ref = config_guard.as_deref();

            let result = dispatch_domain_query(payload, &read_db, config_ref);
            let response = crate::meta::protocol::AuthenticatedResponse::Query(
                Box::new(crate::meta::protocol::QueryResponse::Domain(result)),
            );
            let _ = reply.send(Ok(response));
        });
    }

    /// No-op — each query opens a fresh connection and sees latest committed state.
    pub(super) fn invalidate_scope(&self, _scope: crate::meta::recomputation::RecomputationScope) {}

    /// No-op — next spawned query opens a fresh connection automatically.
    pub(super) fn reconnect_db(&self) {}

    /// Update the cache size applied to new query connections.
    pub(crate) fn set_cache_size(&mut self, kb: i64) {
        self.cache_size_kb = Some(kb);
    }

    /// Store the shared config handle. Queries clone the Arc on spawn.
    pub(super) fn set_config(&mut self, config: crate::config::SharedConfig) {
        self.shared_config = Some(config);
    }

    /// No persistent thread to shut down.
    pub(super) fn shutdown(&mut self) {}
}

impl Drop for CacheThreadHandle {
    fn drop(&mut self) {}
}

// ============================================================================
// Construction
// ============================================================================

/// Create the query dispatch handle. No background thread is spawned.
pub(super) fn spawn() -> CacheThreadHandle {
    crate::logging::log_general("[QUERY_DISPATCH] Ready (per-query thread spawning)");
    CacheThreadHandle {
        shared_config: None,
        cache_size_kb: None,
    }
}

/// Open a read-only database connection for a single query.
fn open_read_only_db(cache_size_kb: Option<i64>) -> Option<Database> {
    let db_path = config::get_db_path().ok()?;
    let db = Database::open_read_only(&db_path).ok()?;
    if let Some(kb) = cache_size_kb {
        let _ = db.conn().execute_batch(&format!("PRAGMA cache_size = {};", kb));
    }
    Some(db)
}
