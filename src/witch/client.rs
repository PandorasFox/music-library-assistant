//! WitchClient trait — the abstract interface for interacting with the Witch.
//!
//! This trait defines the query and command surface that any client (TUI,
//! HTTP server, future socket client) uses to interact with the Witch.
//!
//! State reads go through `witch_status()` — a transparent read of the
//! Witch's published state machine snapshot. Commands are individual methods
//! that send requests to the Witch for execution.
//!
//! **Not included in this trait** (internal orchestration, concrete Witch only):
//! - `tick()` — the Witch's work loop (she drives herself)
//!
//! See `docs/CLIENT_SERVER_ARCHITECTURE.md` for the full design.

use std::path::PathBuf;

use crate::auth::SessionToken;
use crate::meta::decisions::{DecisionKey, DiscardSummary, WitnessedDecision};
use crate::meta::mutations::Mutation;
use crate::meta::protocol::ProtocolError;
use crate::witch::WitchStatus;

/// Detail record for a single decision in the active transaction.
///
/// Heavier than what's in `TransactionSnapshot` — includes mutation data
/// needed for the transaction review view's diff display.
#[derive(Debug, Clone)]
pub struct DecisionDetail {
    pub key: DecisionKey,
    pub label: String,
    pub mutations: Vec<Mutation>,
}

/// Abstract client interface for the Witch.
///
/// The state machine is read via `witch_status()` — a snapshot of all
/// observable Witch state. Commands are methods that mutate state.
///
/// All methods that route through the server return `Result<T, ProtocolError>`.
/// ProtocolError subsumes TransactionError (via `ProtocolError::Transaction`),
/// and adds auth errors (InvalidSession, Unauthorized) and server errors
/// (NotReady, Internal).
pub trait WitchClient {
    // ====================================================================
    // State machine read
    // ====================================================================

    /// Read the Witch's current state machine snapshot.
    ///
    /// For the direct (in-process) impl, this builds the snapshot from
    /// live fields. For the handle impl, this reads from shared memory
    /// (`Arc<RwLock<WitchStatus>>`) — transparent, always fresh.
    fn witch_status(&self) -> WitchStatus;

    // ====================================================================
    // Setup commands
    // ====================================================================

    /// Complete first-time setup: write config, create dirs + DB, create first user, transition to Ready.
    ///
    /// Called by the client after the operator selects an archive root path and
    /// creates the first user account. `first_user` is `(username, password_hash)`.
    ///
    /// Auth-gated: only valid when system is in FirstTimeSetup state.
    fn complete_setup(
        &mut self,
        root: PathBuf,
        first_user: Option<(String, String)>,
    ) -> Result<(), ProtocolError>;

    // ====================================================================
    // Auth
    // ====================================================================

    /// Attempt login. Returns a session token on success.
    fn login(&mut self, username: &str, password: &str) -> Result<SessionToken, String>;

    // ====================================================================
    // Transaction commands
    // ====================================================================

    /// Open a new transaction with the given label.
    fn start_transaction(&mut self, label: &str) -> Result<(), ProtocolError>;

    /// Stage a decision in the active transaction.
    fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), ProtocolError>;

    /// Remove a staged decision from the active transaction.
    fn remove_decision(&mut self, key: &DecisionKey) -> Result<(), ProtocolError>;

    /// Commit the active transaction (execute all staged mutations).
    fn confirm_transaction(&mut self) -> Result<(), ProtocolError>;

    /// Discard the active transaction without executing.
    fn discard_transaction(&mut self) -> Result<DiscardSummary, ProtocolError>;

    // ====================================================================
    // Transaction queries (heavier than status snapshot)
    // ====================================================================

    /// Get full details for all decisions in the active transaction.
    ///
    /// Returns per-decision mutation data needed for diff display in the
    /// transaction review view. For lightweight access (keys, labels,
    /// counts), use `witch_status().transaction` instead.
    fn transaction_decision_details(&self) -> Result<Vec<DecisionDetail>, ProtocolError>;

    // ====================================================================
    // External fetch commands
    // ====================================================================

    /// Kick off external fetch (AcoustID + MusicBrainz lookups).
    fn request_external_fetch(&mut self) -> Result<(), ProtocolError>;

    /// Kick off release bin-packing computation.
    fn request_release_packing(&mut self) -> Result<(), ProtocolError>;

    // ====================================================================
    // Lifecycle commands
    // ====================================================================

    /// Validate a config blob before staging it as a mutation.
    ///
    /// Returns Ok(()) if valid, Err with details if rejected.
    /// Auth-gated: requires authenticated session.
    fn validate_config(&self, config: &crate::config::Config) -> Result<(), ProtocolError>;

    /// Inject shared config (called during startup).
    fn set_shared_config(&mut self, shared: crate::config::SharedConfig) -> Result<(), ProtocolError>;

    /// Start the filesystem watcher. Returns true if watching started.
    fn start_watching(&mut self) -> Result<bool, ProtocolError>;

    /// Update performance config at runtime (pool resize, cache_size).
    fn update_performance(&mut self, opinions: crate::config::PerformanceOpinions) -> Result<(), ProtocolError>;
}

// ========================================================================
// Concrete implementation for direct (in-process) access
// ========================================================================

impl WitchClient for super::Witch {
    fn witch_status(&self) -> WitchStatus {
        self.publish_status()
    }

    fn login(&mut self, username: &str, password: &str) -> Result<SessionToken, String> {
        match &self.auth_handle {
            Some(auth) => auth.login(
                username,
                password,
                crate::auth::SessionLifetime::CloseOnExit,
            ),
            None => Err("Auth not available".to_string()),
        }
    }

    fn complete_setup(
        &mut self,
        root: PathBuf,
        first_user: Option<(String, String)>,
    ) -> Result<(), ProtocolError> {
        self.complete_setup_impl(root, first_user)
            .map_err(|e| ProtocolError::Internal(e))
    }

    fn start_transaction(&mut self, label: &str) -> Result<(), ProtocolError> {
        self.start_transaction(label).map_err(ProtocolError::Transaction)
    }

    fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), ProtocolError> {
        self.add_decision(key, decision).map_err(ProtocolError::Transaction)
    }

    fn remove_decision(&mut self, key: &DecisionKey) -> Result<(), ProtocolError> {
        self.remove_decision(key).map_err(ProtocolError::Transaction)
    }

    fn confirm_transaction(&mut self) -> Result<(), ProtocolError> {
        self.confirm_transaction().map_err(ProtocolError::Transaction)
    }

    fn discard_transaction(&mut self) -> Result<DiscardSummary, ProtocolError> {
        self.discard_transaction().map_err(ProtocolError::Transaction)
    }

    fn transaction_decision_details(&self) -> Result<Vec<DecisionDetail>, ProtocolError> {
        Ok(self.pending_transaction
            .as_ref()
            .map(|txn| {
                txn.decisions
                    .iter()
                    .map(|(k, d)| DecisionDetail {
                        key: k.clone(),
                        label: d.label.clone(),
                        mutations: d.mutations.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn request_external_fetch(&mut self) -> Result<(), ProtocolError> {
        self.request_external_fetch();
        Ok(())
    }

    fn request_release_packing(&mut self) -> Result<(), ProtocolError> {
        self.request_release_packing();
        Ok(())
    }

    fn validate_config(&self, config: &crate::config::Config) -> Result<(), ProtocolError> {
        config.validate().map_err(|e| ProtocolError::Internal(format!("{:#}", e)))
    }

    fn set_shared_config(&mut self, shared: crate::config::SharedConfig) -> Result<(), ProtocolError> {
        self.set_shared_config(shared);
        Ok(())
    }

    fn start_watching(&mut self) -> Result<bool, ProtocolError> {
        Ok(self.start_watching())
    }

    fn update_performance(&mut self, opinions: crate::config::PerformanceOpinions) -> Result<(), ProtocolError> {
        self.update_performance_impl(opinions);
        Ok(())
    }
}
