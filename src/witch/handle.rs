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

use std::sync::mpsc;

use std::path::PathBuf;

use crate::auth::SessionToken;
use crate::config::SharedConfig;
use crate::meta::decisions::{DecisionKey, DiscardSummary, WitnessedDecision};
use crate::meta::protocol::{
    AuthResponse, AuthenticatedBody, AuthenticatedResponse, CommandPayload, CommandResponse,
    DecisionDetail, ProtocolError, ProtocolQuery, StatusQuery,
    TransactionPayload, TransactionResponse, UnauthenticatedBody, UnauthenticatedResponse,
};

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
// WitchHandle
// ============================================================================

/// Client handle for a self-owning Witch running in her own thread.
///
/// The single interface between clients and the Witch. All communication
/// goes through typed protocol messages — no shared memory, no bypass paths.
pub struct WitchHandle {
    /// Command channel to the Witch thread.
    cmd_tx: mpsc::Sender<HandleCommand>,

    /// Session token from the authenticated operator. Set after login.
    /// Threaded into every outgoing HandleCommand for server-side validation.
    session_token: Option<SessionToken>,
}

impl WitchHandle {
    /// Create a new handle from its components.
    pub(super) fn new(
        cmd_tx: mpsc::Sender<HandleCommand>,
    ) -> Self {
        Self {
            cmd_tx,
            session_token: None,
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
    /// Sends a StatusQuery through the protocol — synchronous round-trip to
    /// the Witch thread. The Witch replies immediately from `publish_status()`.
    pub fn witch_status(&self) -> WitchStatus {
        self.send_query(StatusQuery)
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
    // Domain Queries
    // =========================================================================

    /// Submit a domain query through the protocol. Blocks until the result
    /// arrives from the DB read thread.
    ///
    /// Convenience wrapper over `send_query()` — all registered domain queries
    /// implement `ProtocolQuery` via the `domain_query_protocol!` macro.
    pub fn query<Q: ProtocolQuery>(&self, q: Q) -> Q::Response {
        self.send_query(q)
    }
}

impl Drop for WitchHandle {
    fn drop(&mut self) {
        // Best-effort shutdown signal. If the channel is already closed
        // (Witch panicked), that's fine — we're dropping anyway.
        let _ = self.cmd_tx.send(HandleCommand::Shutdown);
    }
}
