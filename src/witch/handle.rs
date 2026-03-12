//! WitchHandle — client handle for interacting with the self-owning Witch.
//!
//! The Witch owns the main thread. Clients (e.g. TUI) run in spawned threads
//! and communicate via:
//! - `Arc<RwLock<WitchStatus>>` for transparent state reads (no cache, no staleness)
//! - `mpsc::Sender<HandleCommand>` for commands (with oneshot response channels)
//!
//! See `docs/CLIENT_SERVER_ARCHITECTURE.md` for the full design.

use std::sync::mpsc;
use std::sync::{Arc, RwLock};

use std::path::PathBuf;

use crate::config::SharedConfig;
use crate::meta::decisions::{DecisionKey, DiscardSummary, TransactionError, WitnessedDecision};

use super::client::{DecisionDetail, WitchClient};
use super::WitchStatus;

// ============================================================================
// Command types (handle → Witch thread)
// ============================================================================

/// A command sent from the handle to the Witch thread.
///
/// Each variant that needs a response carries a oneshot sender.
pub(super) enum HandleCommand {
    // -- Transaction commands --
    StartTransaction {
        label: String,
        reply: mpsc::Sender<Result<(), TransactionError>>,
    },
    AddDecision {
        key: DecisionKey,
        decision: WitnessedDecision,
        reply: mpsc::Sender<Result<(), TransactionError>>,
    },
    RemoveDecision {
        key: DecisionKey,
        reply: mpsc::Sender<Result<(), TransactionError>>,
    },
    ConfirmTransaction {
        reply: mpsc::Sender<Result<(), TransactionError>>,
    },
    DiscardTransaction {
        reply: mpsc::Sender<Result<DiscardSummary, TransactionError>>,
    },

    // -- Transaction queries --
    GetTransactionDetails {
        reply: mpsc::Sender<Vec<DecisionDetail>>,
    },

    // -- External fetch --
    RequestExternalFetch,
    RequestReleasePacking,

    // -- Setup --
    CompleteSetup {
        root: PathBuf,
        reply: mpsc::Sender<Result<(), String>>,
    },

    // -- Maintenance / lifecycle --
    QueueSchemaReconciliation,
    QueueVacuum,
    LatchReadOnlyForSafety { reason: String },
    SetSharedConfig { shared: SharedConfig },
    StartWatching {
        reply: mpsc::Sender<bool>,
    },

    // -- Shutdown --
    Shutdown,
}

// ============================================================================
// WitchHandle
// ============================================================================

/// Client handle for a self-owning Witch running in her own thread.
///
/// State reads go through shared memory (`Arc<RwLock<WitchStatus>>`).
/// Commands go through a channel with optional oneshot reply.
pub struct WitchHandle {
    /// Transparent read into the Witch's state machine.
    /// Updated by the Witch each tick. Reads are lock-free in practice
    /// (write contention is one update per tick, ~10ms).
    status: Arc<RwLock<WitchStatus>>,

    /// Command channel to the Witch thread.
    cmd_tx: mpsc::Sender<HandleCommand>,
}

impl WitchHandle {
    /// Create a new handle from its components.
    pub(super) fn new(
        status: Arc<RwLock<WitchStatus>>,
        cmd_tx: mpsc::Sender<HandleCommand>,
    ) -> Self {
        Self { status, cmd_tx }
    }

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

    /// Send a fire-and-forget command (no reply expected).
    fn send(&self, cmd: HandleCommand) {
        self.cmd_tx
            .send(cmd)
            .expect("Witch thread has shut down unexpectedly");
    }
}

impl WitchClient for WitchHandle {
    fn witch_status(&self) -> WitchStatus {
        self.status
            .read()
            .expect("WitchStatus lock poisoned")
            .clone()
    }

    fn complete_setup(&mut self, root: PathBuf) -> Result<(), String> {
        self.send_recv(|reply| HandleCommand::CompleteSetup { root, reply })
    }

    fn start_transaction(&mut self, label: &str) -> Result<(), TransactionError> {
        self.send_recv(|reply| HandleCommand::StartTransaction {
            label: label.to_owned(),
            reply,
        })
    }

    fn add_decision(
        &mut self,
        key: DecisionKey,
        decision: WitnessedDecision,
    ) -> Result<(), TransactionError> {
        self.send_recv(|reply| HandleCommand::AddDecision {
            key,
            decision,
            reply,
        })
    }

    fn remove_decision(&mut self, key: &DecisionKey) -> Result<(), TransactionError> {
        self.send_recv(|reply| HandleCommand::RemoveDecision {
            key: key.clone(),
            reply,
        })
    }

    fn confirm_transaction(&mut self) -> Result<(), TransactionError> {
        self.send_recv(|reply| HandleCommand::ConfirmTransaction { reply })
    }

    fn discard_transaction(&mut self) -> Result<DiscardSummary, TransactionError> {
        self.send_recv(|reply| HandleCommand::DiscardTransaction { reply })
    }

    fn transaction_decision_details(&self) -> Vec<DecisionDetail> {
        self.send_recv(|reply| HandleCommand::GetTransactionDetails { reply })
    }

    fn request_external_fetch(&mut self) {
        self.send(HandleCommand::RequestExternalFetch);
    }

    fn request_release_packing(&mut self) {
        self.send(HandleCommand::RequestReleasePacking);
    }

    fn queue_schema_reconciliation(&mut self) {
        self.send(HandleCommand::QueueSchemaReconciliation);
    }

    fn queue_vacuum(&mut self) {
        self.send(HandleCommand::QueueVacuum);
    }

    fn latch_read_only_for_safety(&mut self, reason: String) {
        self.send(HandleCommand::LatchReadOnlyForSafety { reason });
    }

    fn set_shared_config(&mut self, shared: SharedConfig) {
        self.send(HandleCommand::SetSharedConfig { shared });
    }

    fn start_watching(&mut self) -> bool {
        self.send_recv(|reply| HandleCommand::StartWatching { reply })
    }
}

impl Drop for WitchHandle {
    fn drop(&mut self) {
        // Best-effort shutdown signal. If the channel is already closed
        // (Witch panicked), that's fine — we're dropping anyway.
        let _ = self.cmd_tx.send(HandleCommand::Shutdown);
    }
}
