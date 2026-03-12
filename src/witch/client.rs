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

use crate::meta::decisions::{DecisionKey, DiscardSummary, TransactionError, WitnessedDecision};
use crate::meta::mutations::Mutation;
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
    // Transaction commands
    // ====================================================================

    /// Open a new transaction with the given label.
    fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError>;

    /// Stage a decision in the active transaction.
    fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), TransactionError>;

    /// Remove a staged decision from the active transaction.
    fn remove_decision(&mut self, key: &DecisionKey) -> Result<(), TransactionError>;

    /// Commit the active transaction (execute all staged mutations).
    fn confirm_transaction(&mut self) -> Result<(), TransactionError>;

    /// Discard the active transaction without executing.
    fn discard_transaction(&mut self) -> Result<DiscardSummary, TransactionError>;

    // ====================================================================
    // Transaction queries (heavier than status snapshot)
    // ====================================================================

    /// Get full details for all decisions in the active transaction.
    ///
    /// Returns per-decision mutation data needed for diff display in the
    /// transaction review view. For lightweight access (keys, labels,
    /// counts), use `witch_status().transaction` instead.
    fn transaction_decision_details(&self) -> Vec<DecisionDetail>;

    // ====================================================================
    // External fetch commands
    // ====================================================================

    /// Kick off external fetch (AcoustID + MusicBrainz lookups).
    fn request_external_fetch(&mut self);

    /// Kick off release bin-packing computation.
    fn request_release_packing(&mut self);

    // ====================================================================
    // Lifecycle commands
    // ====================================================================

    /// Queue schema reconciliation for execution.
    fn queue_schema_reconciliation(&mut self);

    /// Queue a VACUUM operation on the database.
    fn queue_vacuum(&mut self);

    /// Latch the Witch into read-only mode for safety.
    fn latch_read_only_for_safety(&mut self, reason: String);

    /// Inject shared config (called during startup).
    fn set_shared_config(&mut self, shared: crate::config::SharedConfig);

    /// Start the filesystem watcher. Returns true if watching started.
    fn start_watching(&mut self) -> bool;
}

// ========================================================================
// Concrete implementation for direct (in-process) access
// ========================================================================

impl WitchClient for super::Witch {
    fn witch_status(&self) -> WitchStatus {
        self.publish_status()
    }

    fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError> {
        self.start_transaction(label)
    }

    fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), TransactionError> {
        self.add_decision(key, decision)
    }

    fn remove_decision(&mut self, key: &DecisionKey) -> Result<(), TransactionError> {
        self.remove_decision(key)
    }

    fn confirm_transaction(&mut self) -> Result<(), TransactionError> {
        self.confirm_transaction()
    }

    fn discard_transaction(&mut self) -> Result<DiscardSummary, TransactionError> {
        self.discard_transaction()
    }

    fn transaction_decision_details(&self) -> Vec<DecisionDetail> {
        self.pending_transaction
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
            .unwrap_or_default()
    }

    fn request_external_fetch(&mut self) {
        self.request_external_fetch()
    }

    fn request_release_packing(&mut self) {
        self.request_release_packing()
    }

    fn queue_schema_reconciliation(&mut self) {
        self.queue_schema_reconciliation()
    }

    fn queue_vacuum(&mut self) {
        self.queue_vacuum()
    }

    fn latch_read_only_for_safety(&mut self, reason: String) {
        self.latch_read_only_for_safety(reason)
    }

    fn set_shared_config(&mut self, shared: crate::config::SharedConfig) {
        self.set_shared_config(shared)
    }

    fn start_watching(&mut self) -> bool {
        self.start_watching()
    }
}
