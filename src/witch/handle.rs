//! WitchHandle — the single client handle for interacting with the Witch.
//!
//! The Witch owns the main thread. Clients (e.g. TUI) run in spawned threads
//! and communicate via:
//! - `Arc<RwLock<WitchStatus>>` for transparent state reads (no cache, no staleness)
//! - `mpsc::Sender<HandleCommand>` for protocol-routed commands
//! - Cache channels for periodic + one-shot DB queries
//!
//! All server-side concepts (cache thread, auth thread, DB connections) are
//! encapsulated here. The UI sees only WitchHandle.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, RwLock};

use std::path::PathBuf;

use crate::auth::SessionToken;
use crate::config::SharedConfig;
use crate::db::domain::{CachedQuery, DomainQuery};
use crate::meta::decisions::{DecisionKey, DiscardSummary, WitnessedDecision};
use crate::meta::protocol::{
    AuthResponse, AuthenticatedBody, AuthenticatedResponse, CommandPayload, CommandResponse,
    DecisionDetail, ProtocolError, ProtocolQuery,
    TransactionPayload, TransactionResponse, UnauthenticatedBody, UnauthenticatedResponse,
};

use super::cache_thread::CacheHandle;
use super::WitchStatus;

// ============================================================================
// Command types (handle → Witch thread)
// ============================================================================

/// A command sent from the handle to the Witch thread.
///
/// Three variants: authenticated protocol request, unauthenticated protocol
/// request, and lifecycle (shutdown/notify). The reply channel carries the
/// area-matching response type.
pub(super) enum HandleCommand {
    /// Authenticated protocol request (queries, transactions, commands).
    Authenticated {
        token: SessionToken,
        body: AuthenticatedBody,
        reply: mpsc::Sender<Result<AuthenticatedResponse, ProtocolError>>,
    },

    /// Unauthenticated protocol request (login, setup).
    Unauthenticated {
        body: UnauthenticatedBody,
        reply: mpsc::Sender<Result<UnauthenticatedResponse, ProtocolError>>,
    },

    /// Notify auth thread that DB is now available (after first-time setup).
    NotifyDbReady,

    /// Shutdown the Witch.
    Shutdown,
}

// ============================================================================
// PendingQuery<T> (typed one-shot response)
// ============================================================================

/// A pending one-shot query result.
///
/// Wraps a channel receiver. The query runs on the cache thread;
/// the result arrives when complete.
pub struct PendingQuery<T> {
    rx: Receiver<T>,
}

impl<T> PendingQuery<T> {
    /// Blocking wait for the result.
    pub fn recv(self) -> T {
        self.rx.recv().expect("cache thread dropped query sender")
    }

    /// Non-blocking poll for the result.
    ///
    /// Returns `Ok(value)` if ready, `Err(self)` if still pending (returns self
    /// back so you can try again next frame).
    pub fn try_recv(self) -> Result<T, Self> {
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
// CachedData (type-erased periodic query cache)
// ============================================================================

/// Type-erased cache for periodic query results from the cache thread.
///
/// Keyed by `TypeId` of the `CachedQuery` implementor. Values are the
/// query's `Response` type, boxed and downcast on access.
pub struct CachedData {
    slots: HashMap<TypeId, Box<dyn Any + Send>>,
}

impl CachedData {
    fn new() -> Self {
        Self {
            slots: HashMap::new(),
        }
    }

    /// Get a cached query result by query type.
    pub fn get<Q: CachedQuery>(&self) -> Option<&Q::Response> {
        self.slots
            .get(&TypeId::of::<Q>())
            .and_then(|v| v.downcast_ref::<Q::Response>())
    }

    /// Insert a raw cache result (called from drain loop).
    fn insert_raw(&mut self, type_id: TypeId, value: Box<dyn Any + Send>) {
        self.slots.insert(type_id, value);
    }
}

// ============================================================================
// WitchHandle
// ============================================================================

/// Client handle for a self-owning Witch running in her own thread.
///
/// The single interface between clients and the Witch. Encapsulates:
/// - Transparent state reads (`Arc<RwLock<WitchStatus>>`)
/// - Protocol-routed command channel to the Witch thread
/// - Cache thread channels (periodic + one-shot DB queries)
/// - Session token management
/// - Type-erased periodic query cache
pub struct WitchHandle {
    /// Transparent read into the Witch's state machine.
    /// Updated by the Witch each tick. Reads are lock-free in practice
    /// (write contention is one update per tick, ~10ms).
    status: Arc<RwLock<WitchStatus>>,

    /// Command channel to the Witch thread.
    cmd_tx: mpsc::Sender<HandleCommand>,

    /// Session token from the authenticated operator. Set after login.
    /// Threaded into every outgoing HandleCommand for server-side validation.
    session_token: Option<SessionToken>,

    // -- Cache thread channels --
    /// Send cache requests (want, query, invalidate).
    cache: CacheHandle,

    /// Locally cached periodic data from the cache thread (TypeId-keyed).
    pub cached: CachedData,

    /// Set when `MutationsCompleted` fires (cache invalidated), cleared when
    /// fresh `CacheReady` results arrive. Views stay greyed-out while true so
    /// the operator never sees stale counts with an interactive overlay.
    cache_stale: bool,
}

impl WitchHandle {
    /// Create a new handle from its components.
    pub(super) fn new(
        status: Arc<RwLock<WitchStatus>>,
        cmd_tx: mpsc::Sender<HandleCommand>,
        cache: CacheHandle,
    ) -> Self {
        Self {
            status,
            cmd_tx,
            session_token: None,
            cache,
            cached: CachedData::new(),
            cache_stale: false,
        }
    }

    /// Notify the Witch that the DB is now available (after first-time setup).
    pub fn notify_db_ready(&self) {
        let _ = self.cmd_tx.send(HandleCommand::NotifyDbReady);
    }

    // =========================================================================
    // Protocol: Authenticated requests
    // =========================================================================

    /// Send an authenticated request and wait for the reply.
    /// Panics if no session token is set (call login() first).
    fn send_authenticated(
        &self,
        body: AuthenticatedBody,
    ) -> Result<AuthenticatedResponse, ProtocolError> {
        let token = self
            .session_token
            .clone()
            .expect("send_authenticated called before login");
        let (tx, rx) = mpsc::channel();
        self.cmd_tx
            .send(HandleCommand::Authenticated {
                token,
                body,
                reply: tx,
            })
            .expect("Witch thread has shut down unexpectedly");
        rx.recv()
            .expect("Witch thread dropped reply channel unexpectedly")
    }

    /// Send a typed protocol query and extract the typed response.
    ///
    /// Panics on protocol errors (in-process, these indicate bugs).
    /// Callsite usage: `let status: WitchStatus = handle.send_query(StatusQuery);`
    pub fn send_query<Q: ProtocolQuery>(&self, q: Q) -> Q::Response {
        let body = AuthenticatedBody::Query(q.into_payload());
        match self.send_authenticated(body) {
            Ok(AuthenticatedResponse::Query(qr)) => Q::extract_response(qr),
            Ok(_) => unreachable!("protocol bug: wrong response area"),
            Err(e) => panic!("protocol query failed: {e}"),
        }
    }

    /// Send a transaction operation and return the transaction response.
    pub fn send_transaction(
        &self,
        payload: TransactionPayload,
    ) -> TransactionResponse {
        let body = AuthenticatedBody::Transaction(payload);
        match self.send_authenticated(body) {
            Ok(AuthenticatedResponse::Transaction(tr)) => tr,
            Ok(_) => unreachable!("protocol bug: wrong response area"),
            Err(_) => TransactionResponse::Error(
                crate::meta::decisions::TransactionError::NotAcceptingMutations,
            ),
        }
    }

    /// Send a command and return the command response.
    pub fn send_command(&self, payload: CommandPayload) -> Result<CommandResponse, ProtocolError> {
        let body = AuthenticatedBody::Command(payload);
        match self.send_authenticated(body)? {
            AuthenticatedResponse::Command(cr) => Ok(cr),
            _ => unreachable!("protocol bug: wrong response area"),
        }
    }

    // =========================================================================
    // Protocol: Unauthenticated requests
    // =========================================================================

    /// Send an unauthenticated request and wait for the reply.
    fn send_unauthenticated(
        &self,
        body: UnauthenticatedBody,
    ) -> Result<UnauthenticatedResponse, ProtocolError> {
        let (tx, rx) = mpsc::channel();
        self.cmd_tx
            .send(HandleCommand::Unauthenticated { body, reply: tx })
            .expect("Witch thread has shut down unexpectedly");
        rx.recv()
            .expect("Witch thread dropped reply channel unexpectedly")
    }

    // =========================================================================
    // Convenience methods (ergonomic sugar over protocol)
    // =========================================================================

    /// Read the Witch's current state machine snapshot.
    ///
    /// Reads from shared memory (`Arc<RwLock<WitchStatus>>`) — transparent,
    /// always fresh, sub-microsecond.
    pub fn witch_status(&self) -> WitchStatus {
        self.status
            .read()
            .expect("WitchStatus lock poisoned")
            .clone()
    }

    /// Attempt login. Returns a session token on success.
    pub fn login(&mut self, username: &str, password: &str) -> Result<SessionToken, String> {
        let response = self.send_unauthenticated(UnauthenticatedBody::Login {
            username: username.to_string(),
            password: password.to_string(),
        });
        match response {
            Ok(UnauthenticatedResponse::Auth(AuthResponse::Token(token))) => {
                self.session_token = Some(token.clone());
                Ok(token)
            }
            Ok(UnauthenticatedResponse::Auth(AuthResponse::Failed(msg))) => Err(msg),
            Ok(_) => Err("unexpected response to login".to_string()),
            Err(e) => Err(format!("{e}")),
        }
    }

    /// Complete first-time setup.
    pub fn complete_setup(
        &mut self,
        root: PathBuf,
        first_user: Option<(String, String)>,
    ) -> Result<(), ProtocolError> {
        match self.send_unauthenticated(UnauthenticatedBody::CompleteSetup { root, first_user })? {
            UnauthenticatedResponse::SetupComplete => Ok(()),
            _ => Err(ProtocolError::Internal(
                "unexpected response to setup".to_string(),
            )),
        }
    }

    /// Open a new transaction.
    pub fn start_transaction(&mut self, label: &str) -> Result<(), ProtocolError> {
        match self.send_transaction(TransactionPayload::Start {
            label: label.to_owned(),
        }) {
            TransactionResponse::Ok => Ok(()),
            TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Stage a decision in the active transaction.
    pub fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), ProtocolError> {
        match self.send_transaction(TransactionPayload::AddDecision { key, decision }) {
            TransactionResponse::Ok => Ok(()),
            TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Remove a staged decision.
    pub fn remove_decision(&mut self, key: &DecisionKey) -> Result<(), ProtocolError> {
        match self.send_transaction(TransactionPayload::RemoveDecision { key: key.clone() }) {
            TransactionResponse::Ok => Ok(()),
            TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Commit the active transaction.
    pub fn confirm_transaction(&mut self) -> Result<(), ProtocolError> {
        match self.send_transaction(TransactionPayload::Confirm) {
            TransactionResponse::Ok => Ok(()),
            TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Discard the active transaction.
    pub fn discard_transaction(&mut self) -> Result<DiscardSummary, ProtocolError> {
        match self.send_transaction(TransactionPayload::Discard) {
            TransactionResponse::Discarded(s) => Ok(s),
            TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Get full details for all decisions in the active transaction.
    pub fn transaction_decision_details(&self) -> Result<Vec<DecisionDetail>, ProtocolError> {
        match self.send_transaction(TransactionPayload::GetDetails) {
            TransactionResponse::Details(d) => Ok(d),
            TransactionResponse::Error(e) => Err(ProtocolError::Transaction(e)),
            _ => unreachable!("protocol bug: wrong transaction response"),
        }
    }

    /// Kick off external fetch.
    pub fn request_external_fetch(&mut self) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::RequestExternalFetch)?;
        Ok(())
    }

    /// Kick off release bin-packing.
    pub fn request_release_packing(&mut self) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::RequestReleasePacking)?;
        Ok(())
    }

    /// Validate a config blob.
    pub fn validate_config(&self, config: &crate::config::Config) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::ValidateConfig {
            config: config.clone(),
        })?;
        Ok(())
    }

    /// Inject shared config.
    pub fn set_shared_config(&mut self, shared: SharedConfig) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::SetSharedConfig { shared })?;
        Ok(())
    }

    /// Start the filesystem watcher.
    pub fn start_watching(&mut self) -> Result<bool, ProtocolError> {
        match self.send_command(CommandPayload::StartWatching)? {
            CommandResponse::WatchingStarted(v) => Ok(v),
            CommandResponse::Ok => Ok(false),
        }
    }

    /// Update performance config at runtime.
    pub fn update_performance(
        &mut self,
        opinions: crate::config::PerformanceOpinions,
    ) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::UpdatePerformance { opinions })?;
        Ok(())
    }

    // =========================================================================
    // Cache: one-shot queries
    // =========================================================================

    /// Submit a blocking one-shot domain query. Returns the result directly.
    pub fn query<Q: DomainQuery>(&self, q: Q) -> Q::Response {
        self.cache.domain_query(q).recv()
    }

    /// Submit an async one-shot domain query. Returns a `PendingQuery` to poll.
    pub fn query_async<Q: DomainQuery>(&self, q: Q) -> PendingQuery<Q::Response> {
        let inner = self.cache.domain_query(q);
        PendingQuery { rx: inner.rx }
    }

    // =========================================================================
    // Cache: periodic subscriptions
    // =========================================================================

    /// Signal demand for a cached query (normal throttle).
    pub fn subscribe<Q: CachedQuery>(&self) {
        self.cache.want::<Q>();
    }

    /// Signal urgent demand for a cached query (uses urgent throttle if defined).
    pub fn subscribe_urgent<Q: CachedQuery>(&self) {
        self.cache.want_urgent::<Q>();
    }

    /// Drain ready results from the cache thread into the local cache.
    ///
    /// Call once per frame. Returns the list of TypeIds that arrived
    /// (for post-drain side effects like updating tree browser markers).
    pub fn drain_subscriptions(&mut self) -> Vec<TypeId> {
        let ready_items = self.cache.drain_ready();
        if ready_items.is_empty() {
            return Vec::new();
        }
        self.cache_stale = false;
        let mut arrivals = Vec::with_capacity(ready_items.len());
        for item in ready_items {
            arrivals.push(item.type_id);
            self.cached.insert_raw(item.type_id, item.value);
        }
        arrivals
    }

    /// Whether cached data is stale (mutations completed, fresh results not yet arrived).
    pub fn is_cache_stale(&self) -> bool {
        self.cache_stale
    }

    /// Mark cache as stale. Called when MutationsCompleted notice arrives.
    pub fn set_cache_stale(&mut self) {
        self.cache_stale = true;
    }

    /// Tell the cache thread to invalidate entries whose scope overlaps.
    pub fn invalidate_cache(&self, scope: crate::meta::recomputation::RecomputationScope) {
        self.cache.invalidate_scope(scope);
    }

    /// Tell the cache thread to close and reopen its DB connection.
    /// Used after schema migrations.
    pub fn reconnect_cache_db(&self) {
        self.cache.reconnect_db();
    }
}

impl Drop for WitchHandle {
    fn drop(&mut self) {
        // Best-effort shutdown signal. If the channel is already closed
        // (Witch panicked), that's fine — we're dropping anyway.
        let _ = self.cmd_tx.send(HandleCommand::Shutdown);
    }
}
