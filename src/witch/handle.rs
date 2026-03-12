//! WitchHandle — the single client handle for interacting with the Witch.
//!
//! The Witch owns the main thread. Clients (e.g. TUI) run in spawned threads
//! and communicate via:
//! - `Arc<RwLock<WitchStatus>>` for transparent state reads (no cache, no staleness)
//! - `mpsc::Sender<HandleCommand>` for commands (with oneshot response channels)
//! - Cache channels for periodic + one-shot DB queries
//!
//! All server-side concepts (cache thread, auth thread, DB connections) are
//! encapsulated here. The UI sees only WitchHandle.
//!
//! See `docs/CLIENT_SERVER_ARCHITECTURE.md` for the full design.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, RwLock};

use std::path::PathBuf;

use crate::auth::SessionToken;
use crate::config::SharedConfig;
use crate::db::domain::{CachedQuery, DomainQuery};
use crate::meta::decisions::{DecisionKey, DiscardSummary, WitnessedDecision};
use crate::meta::protocol::{AuthRequest, AuthResponse, CommandResponse, ProtocolError, WitchCommand};

use super::cache_thread::CacheHandle;
use super::client::{DecisionDetail, WitchClient};
use super::WitchStatus;

// ============================================================================
// Command types (handle → Witch thread)
// ============================================================================

/// A command sent from the handle to the Witch thread.
///
/// Each variant that needs a response carries a oneshot sender.
/// Protocol commands carry a session token for server-side auth gating.
pub(super) enum HandleCommand {
    // -- Protocol dispatch (auth-gated via token) --
    Command {
        token: Option<SessionToken>,
        command: WitchCommand,
        reply: mpsc::Sender<Result<CommandResponse, ProtocolError>>,
    },

    // -- Enriched in-process (carries non-serializable data, still auth-gated) --
    AddDecision {
        token: Option<SessionToken>,
        key: DecisionKey,
        decision: WitnessedDecision,
        reply: mpsc::Sender<Result<(), ProtocolError>>,
    },
    GetTransactionDetails {
        token: Option<SessionToken>,
        reply: mpsc::Sender<Result<Vec<DecisionDetail>, ProtocolError>>,
    },

    // -- Auth (pre-gate — no session token required) --
    Auth {
        request: AuthRequest,
        reply: mpsc::Sender<AuthResponse>,
    },

    /// Notify auth thread that DB is now available (after first-time setup).
    NotifyDbReady,

    // -- Lifecycle (auth-gated) --
    CompleteSetup {
        token: Option<SessionToken>,
        root: PathBuf,
        first_user: Option<(String, String)>,
        reply: mpsc::Sender<Result<(), ProtocolError>>,
    },
    ValidateConfig {
        token: Option<SessionToken>,
        config: crate::config::Config,
        reply: mpsc::Sender<Result<(), ProtocolError>>,
    },
    SetSharedConfig {
        token: Option<SessionToken>,
        shared: SharedConfig,
        reply: mpsc::Sender<Result<(), ProtocolError>>,
    },
    StartWatching {
        token: Option<SessionToken>,
        reply: mpsc::Sender<Result<bool, ProtocolError>>,
    },
    UpdatePerformance {
        token: Option<SessionToken>,
        opinions: crate::config::PerformanceOpinions,
        reply: mpsc::Sender<Result<(), ProtocolError>>,
    },

    // -- Shutdown --
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
/// - Command channel to the Witch thread
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

    // =========================================================================
    // Internal command plumbing
    // =========================================================================

    /// Send a command and wait for the reply.
    fn send_recv<T>(&self, f: impl FnOnce(mpsc::Sender<T>) -> HandleCommand) -> T {
        let (tx, rx) = mpsc::channel();
        let cmd = f(tx);
        self.cmd_tx
            .send(cmd)
            .expect("Witch thread has shut down unexpectedly");
        rx.recv()
            .expect("Witch thread dropped reply channel unexpectedly")
    }

    /// Send a WitchCommand and decode the CommandResponse.
    fn send_command(&self, command: WitchCommand) -> Result<CommandResponse, ProtocolError> {
        self.send_recv(|reply| HandleCommand::Command {
            token: self.session_token.clone(),
            command,
            reply,
        })
    }

    /// Send a WitchCommand expecting Ok response, mapping TransactionError.
    fn send_command_ok(&self, command: WitchCommand) -> Result<(), ProtocolError> {
        match self.send_command(command)? {
            CommandResponse::Ok => Ok(()),
            CommandResponse::TransactionError(e) => Err(ProtocolError::Transaction(e)),
        }
    }
}

impl WitchClient for WitchHandle {
    fn witch_status(&self) -> WitchStatus {
        self.status
            .read()
            .expect("WitchStatus lock poisoned")
            .clone()
    }

    fn login(&mut self, username: &str, password: &str) -> Result<SessionToken, String> {
        let response = self.send_recv(|reply| HandleCommand::Auth {
            request: AuthRequest::Login {
                username: username.to_string(),
                password: password.to_string(),
            },
            reply,
        });
        match response {
            AuthResponse::Token(token) => {
                self.session_token = Some(token.clone());
                Ok(token)
            }
            AuthResponse::Failed(msg) => Err(msg),
        }
    }

    fn complete_setup(
        &mut self,
        root: PathBuf,
        first_user: Option<(String, String)>,
    ) -> Result<(), ProtocolError> {
        self.send_recv(|reply| HandleCommand::CompleteSetup {
            token: self.session_token.clone(),
            root,
            first_user,
            reply,
        })
    }

    fn start_transaction(&mut self, label: &str) -> Result<(), ProtocolError> {
        self.send_command_ok(WitchCommand::StartTransaction {
            label: label.to_owned(),
        })
    }

    fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), ProtocolError> {
        self.send_recv(|reply| HandleCommand::AddDecision {
            token: self.session_token.clone(),
            key,
            decision,
            reply,
        })
    }

    fn remove_decision(&mut self, key: &DecisionKey) -> Result<(), ProtocolError> {
        self.send_command_ok(WitchCommand::RemoveDecision { key: key.clone() })
    }

    fn confirm_transaction(&mut self) -> Result<(), ProtocolError> {
        self.send_command_ok(WitchCommand::ConfirmTransaction)
    }

    fn discard_transaction(&mut self) -> Result<DiscardSummary, ProtocolError> {
        match self.send_command(WitchCommand::DiscardTransaction)? {
            CommandResponse::Ok => Ok(DiscardSummary),
            CommandResponse::TransactionError(e) => Err(ProtocolError::Transaction(e)),
        }
    }

    fn transaction_decision_details(&self) -> Result<Vec<DecisionDetail>, ProtocolError> {
        self.send_recv(|reply| HandleCommand::GetTransactionDetails {
            token: self.session_token.clone(),
            reply,
        })
    }

    fn request_external_fetch(&mut self) -> Result<(), ProtocolError> {
        self.send_command_ok(WitchCommand::RequestExternalFetch)
    }

    fn request_release_packing(&mut self) -> Result<(), ProtocolError> {
        self.send_command_ok(WitchCommand::RequestReleasePacking)
    }

    fn validate_config(&self, config: &crate::config::Config) -> Result<(), ProtocolError> {
        self.send_recv(|reply| HandleCommand::ValidateConfig {
            token: self.session_token.clone(),
            config: config.clone(),
            reply,
        })
    }

    fn set_shared_config(&mut self, shared: SharedConfig) -> Result<(), ProtocolError> {
        self.send_recv(|reply| HandleCommand::SetSharedConfig {
            token: self.session_token.clone(),
            shared,
            reply,
        })
    }

    fn start_watching(&mut self) -> Result<bool, ProtocolError> {
        self.send_recv(|reply| HandleCommand::StartWatching {
            token: self.session_token.clone(),
            reply,
        })
    }

    fn update_performance(&mut self, opinions: crate::config::PerformanceOpinions) -> Result<(), ProtocolError> {
        self.send_recv(|reply| HandleCommand::UpdatePerformance {
            token: self.session_token.clone(),
            opinions,
            reply,
        })
    }
}

impl Drop for WitchHandle {
    fn drop(&mut self) {
        // Best-effort shutdown signal. If the channel is already closed
        // (Witch panicked), that's fine — we're dropping anyway.
        let _ = self.cmd_tx.send(HandleCommand::Shutdown);
    }
}
