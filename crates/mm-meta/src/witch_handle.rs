//! WitchHandle — the single client handle for interacting with the Witch.
//!
//! The Witch owns the main thread. Clients (e.g. TUI) run in spawned threads
//! and communicate exclusively via protocol messages:
//! - `send_query()` for typed read-only queries (status, domain DB queries)
//! - `send_transaction()` for decision lifecycle
//! - `send_command()` for operational actions
//!
//! All server-side concepts (read thread, auth thread, DB connections) are
//! invisible to clients. The UI sees only WitchHandle.
//!
//! ## Transport
//!
//! WitchHandle supports two transports:
//! - **Channel**: in-process mpsc to the Witch thread (default for TUI)
//! - **Socket**: Unix domain socket with length-prefixed bincode framing
//!   (for out-of-process clients)

use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::auth::SessionToken;
use crate::decisions::{Decision, DecisionKey, DiscardSummary, TransactionError};
use crate::protocol::{
    AuthResponse, AuthenticatedBody, AuthenticatedResponse, CommandPayload, CommandResponse,
    ConfigQuery, DecisionDetail, ProtocolError, ProtocolQuery, StatusQuery, TransactionPayload,
    TransactionResponse, UnauthenticatedBody, UnauthenticatedResponse,
};
use crate::wire::{self, WireRequest, WireResponse};
use crate::witch_types::WitchStatus;

// ============================================================================
// Command types (handle → Witch thread)
// ============================================================================

/// A command sent from the handle to the Witch thread.
///
/// Three variants: authenticated protocol request, unauthenticated protocol
/// request, and lifecycle (shutdown/notify). The reply channel carries the
/// area-matching response type.
pub enum HandleCommand {
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
// Transport
// ============================================================================

/// Transport backing a WitchHandle — either in-process mpsc or Unix socket.
enum Transport {
    /// In-process: direct mpsc channel to the Witch thread.
    Channel(mpsc::Sender<HandleCommand>),
    /// Remote: Unix domain socket with length-prefixed bincode framing.
    /// Mutex satisfies the borrow checker — WitchHandle methods take `&self`,
    /// but socket I/O needs exclusive access. Single-client handle, so no
    /// real contention.
    Socket(std::sync::Mutex<std::os::unix::net::UnixStream>),
}

// ============================================================================
// WitchHandle
// ============================================================================

/// Client handle for a self-owning Witch running in her own thread.
///
/// The single interface between clients and the Witch. All communication
/// goes through typed protocol messages — no shared memory, no bypass paths.
pub struct WitchHandle {
    /// Transport to the Witch (channel or socket).
    transport: Transport,

    /// Session token from the authenticated operator. Set after login.
    /// Threaded into every outgoing HandleCommand for server-side validation.
    session_token: Option<SessionToken>,

    /// Cached config from server. Fetched once after login, invalidated on
    /// config generation change.
    cached_config: Option<crate::config::Config>,
}

impl WitchHandle {
    /// Create a new handle backed by an in-process mpsc channel.
    pub fn new(cmd_tx: mpsc::Sender<HandleCommand>) -> Self {
        Self {
            transport: Transport::Channel(cmd_tx),
            session_token: None,
            cached_config: None,
        }
    }

    /// Default socket path: `$XDG_RUNTIME_DIR/mm.sock`.
    ///
    /// Returns `None` if `XDG_RUNTIME_DIR` is not set.
    pub fn default_socket_path() -> Option<std::path::PathBuf> {
        std::env::var_os("XDG_RUNTIME_DIR")
            .map(|dir| std::path::Path::new(&dir).join("mm.sock"))
    }

    /// Connect to a running Witch over a Unix domain socket.
    pub fn connect(socket_path: &Path) -> std::io::Result<Self> {
        let stream = std::os::unix::net::UnixStream::connect(socket_path)?;
        Ok(Self {
            transport: Transport::Socket(std::sync::Mutex::new(stream)),
            session_token: None,
            cached_config: None,
        })
    }

    /// Notify the Witch that the DB is now available (after first-time setup).
    pub fn notify_db_ready(&self) {
        match &self.transport {
            Transport::Channel(cmd_tx) => {
                let _ = cmd_tx.send(HandleCommand::NotifyDbReady);
            }
            Transport::Socket(stream) => {
                let mut stream = stream.lock().expect("socket mutex poisoned");
                let _ = wire::write_frame(&mut *stream, &WireRequest::NotifyDbReady);
                let _: Result<WireResponse, _> = wire::read_frame(&mut *stream);
            }
        }
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

        match &self.transport {
            Transport::Channel(cmd_tx) => {
                let (tx, rx) = mpsc::channel();
                cmd_tx
                    .send(HandleCommand::Authenticated {
                        token,
                        body,
                        reply: tx,
                    })
                    .expect("Witch thread has shut down unexpectedly");
                rx.recv()
                    .expect("Witch thread dropped reply channel unexpectedly")
            }
            Transport::Socket(stream) => {
                let mut stream = stream.lock().expect("socket mutex poisoned");
                let req = WireRequest::Authenticated { token, body };
                wire::write_frame(&mut *stream, &req)
                    .map_err(|e| ProtocolError::Internal(format!("socket write: {e}")))?;
                let resp: WireResponse = wire::read_frame(&mut *stream)
                    .map_err(|e| ProtocolError::Internal(format!("socket read: {e}")))?;
                match resp {
                    WireResponse::Authenticated(result) => result,
                    _ => Err(ProtocolError::Internal(
                        "unexpected wire response type".to_string(),
                    )),
                }
            }
        }
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
                TransactionError::NotAcceptingMutations,
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
        match &self.transport {
            Transport::Channel(cmd_tx) => {
                let (tx, rx) = mpsc::channel();
                cmd_tx
                    .send(HandleCommand::Unauthenticated { body, reply: tx })
                    .expect("Witch thread has shut down unexpectedly");
                rx.recv()
                    .expect("Witch thread dropped reply channel unexpectedly")
            }
            Transport::Socket(stream) => {
                let mut stream = stream.lock().expect("socket mutex poisoned");
                let req = WireRequest::Unauthenticated(body);
                wire::write_frame(&mut *stream, &req)
                    .map_err(|e| ProtocolError::Internal(format!("socket write: {e}")))?;
                let resp: WireResponse = wire::read_frame(&mut *stream)
                    .map_err(|e| ProtocolError::Internal(format!("socket read: {e}")))?;
                match resp {
                    WireResponse::Unauthenticated(result) => result,
                    _ => Err(ProtocolError::Internal(
                        "unexpected wire response type".to_string(),
                    )),
                }
            }
        }
    }

    // =========================================================================
    // Convenience methods (ergonomic sugar over protocol)
    // =========================================================================

    /// Check whether first-time setup is needed (unauthenticated).
    pub fn needs_setup(&self) -> bool {
        match self.send_unauthenticated(UnauthenticatedBody::SetupQuery) {
            Ok(UnauthenticatedResponse::SetupStatus { needs_setup }) => needs_setup,
            _ => false,
        }
    }

    /// Read the Witch's current state machine snapshot.
    pub fn witch_status(&self) -> WitchStatus {
        self.send_query(StatusQuery)
    }

    /// Get the current config. Fetches from server on first call, then
    /// returns the cached copy. Call `invalidate_config_cache()` when the
    /// config generation counter changes.
    pub fn config(&mut self) -> crate::config::Config {
        if let Some(ref c) = self.cached_config {
            return c.clone();
        }
        let config = self.send_query(ConfigQuery);
        self.cached_config = Some(config.clone());
        config
    }

    /// Drop the cached config so the next `config()` call re-fetches.
    pub fn invalidate_config_cache(&mut self) {
        self.cached_config = None;
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
        decision: Decision,
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

    /// Inject shared config (in-process startup only).
    pub fn set_shared_config(&mut self, config: crate::config::Config) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::SetSharedConfig { config })?;
        Ok(())
    }

    /// Update performance config at runtime.
    pub fn update_performance(
        &mut self,
        opinions: crate::config::PerformanceOpinions,
    ) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::UpdatePerformance { opinions })?;
        Ok(())
    }

    /// Delete edit history for a specific session.
    pub fn jettison_edit_history_session(&self, session_id: &str) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::JettisonEditHistorySession {
            session_id: session_id.to_owned(),
        })?;
        Ok(())
    }

    /// Delete all edit history.
    pub fn jettison_edit_history_all(&self) -> Result<(), ProtocolError> {
        self.send_command(CommandPayload::JettisonEditHistoryAll)?;
        Ok(())
    }

    // =========================================================================
    // Domain Queries
    // =========================================================================

    /// Submit a domain query through the protocol. Blocks until the result
    /// arrives from the DB read thread.
    pub fn query<Q: ProtocolQuery>(&self, q: Q) -> Q::Response {
        self.send_query(q)
    }

    /// Request server shutdown. Returns `Ok(())` after the server acknowledges
    /// with `Goodbye`. The server will stop after sending the response.
    pub fn shutdown(&self) -> Result<(), ProtocolError> {
        match self.send_command(CommandPayload::Shutdown)? {
            CommandResponse::Goodbye => Ok(()),
            CommandResponse::Ok => Err(ProtocolError::Internal(
                "expected Goodbye, got Ok".to_string(),
            )),
        }
    }
}

impl Drop for WitchHandle {
    fn drop(&mut self) {
        // Best-effort shutdown signal. Only meaningful for in-process channel —
        // socket clients don't own the server lifecycle.
        if let Transport::Channel(ref cmd_tx) = self.transport {
            let _ = cmd_tx.send(HandleCommand::Shutdown);
        }
    }
}
