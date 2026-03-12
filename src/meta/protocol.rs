//! Protocol types for the Witch client-server architecture.
//!
//! These types define the wire format between clients (TUI, HTTP server)
//! and the Witch server. All types derive Serialize/Deserialize for
//! transport over the Unix socket protocol.
//!
//! See `docs/CLIENT_SERVER_ARCHITECTURE.md` for the full design.

use serde::{Deserialize, Serialize};

use super::decisions::{DecisionKey, DecisionKeyKind, TransactionError};
use crate::witch::external_fetch::FetchProgress;
use crate::witch::{ReasoningLevel, WorkStatus};

// ============================================================================
// Session Identity
// ============================================================================

/// Opaque session identifier. Obtained via Login, included in every request.
///
/// No sentinel values, no special constants. A `SessionId` is either valid
/// (returned by the Witch after authentication) or it doesn't exist.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SessionId(pub u128);

// ============================================================================
// Authorization
// ============================================================================

/// Authorization level for a client session.
///
/// Determines which commands a session may issue. The Witch checks this
/// on every incoming command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorizationLevel {
    /// No users exist yet. Only setup commands accepted.
    FirstTimeSetup,
    /// Before login. Status queries only (is the system up? needs setup?).
    Unauthenticated,
    /// Valid session token. Everything available. Default operational mode.
    AuthRequired,
}

// ============================================================================
// Queries (read-only requests)
// ============================================================================

/// Read-only queries against the Witch's state.
///
/// Each variant maps to one or more Witch read methods. Queries never
/// mutate state and do not require a `ConfirmationGesture`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WitchQuery {
    /// Current work status snapshot.
    Status,
    /// Current reasoning level (None | Inodes | Full).
    ReasoningLevel,
    /// Whether any decisions are pending in this session's transaction.
    HasPending,
    /// Whether initial corpus scanning is still in progress.
    IsInitialScanning,
    /// Number of tasks queued in the db_thread write queue.
    DbQueueDepth,
    /// Whether this session has an active transaction.
    HasTransaction,
    /// Summary of the active transaction (label, decision count, mutation count).
    TransactionSummary,
    /// All decision keys in the active transaction.
    DecisionKeys,
    /// Get a specific decision's label by key.
    GetDecision(DecisionKey),
    /// Which singleton DecisionKeyKinds are currently handled.
    HandledDecisionKinds,
    /// Whether the external fetch scheduler is currently running.
    IsExternalFetchActive,
    /// Progress snapshot from external fetch (AcoustID + MusicBrainz).
    ExternalFetchProgress,
    /// Whether an AcoustID API key is configured.
    HasAcoustIdApiKey,
    /// Whether the database schema needs updating.
    NeedsSchemaUpdate,
    /// Human-readable descriptions of pending schema changes.
    PendingSchemaDescriptions,
}

// ============================================================================
// Commands (state-mutating requests)
// ============================================================================

/// State-mutating commands sent to the Witch.
///
/// Each command requires a valid session with sufficient authorization.
/// The Witch checks `required_authorization()` before dispatching.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WitchCommand {
    /// Open a new transaction for this session.
    StartTransaction { label: String },
    /// Stage a decision in the active transaction.
    AddDecision {
        key: DecisionKey,
        label: String,
        /// Mutation count (mutations themselves are not serialized over the wire
        /// in Phase 1 — the actual mutation data stays server-side when the TUI
        /// is a direct client. This field exists for protocol completeness).
        mutation_count: usize,
    },
    /// Remove a staged decision from the active transaction.
    RemoveDecision { key: DecisionKey },
    /// Commit the active transaction (execute all staged mutations).
    ConfirmTransaction,
    /// Discard the active transaction without executing.
    DiscardTransaction,
    /// Kick off external fetch (AcoustID + MusicBrainz lookups).
    RequestExternalFetch,
    /// Kick off release bin-packing computation.
    RequestReleasePacking,
    /// Queue schema reconciliation (requires operator approval).
    QueueSchemaReconciliation,
    /// Queue a VACUUM operation on the database.
    QueueVacuum,
    /// Latch the Witch into read-only mode for safety.
    LatchReadOnlyForSafety { reason: String },
}

impl WitchQuery {
    /// The minimum authorization level required to execute this query.
    ///
    /// Schema queries are available pre-login (the TUI needs to know
    /// whether to prompt for schema reconciliation before auth).
    /// Everything else requires a valid session.
    pub fn required_authorization(&self) -> AuthorizationLevel {
        match self {
            WitchQuery::NeedsSchemaUpdate | WitchQuery::PendingSchemaDescriptions => {
                AuthorizationLevel::Unauthenticated
            }
            // Catch-all: any new variant defaults to AuthRequired.
            _ => AuthorizationLevel::AuthRequired,
        }
    }
}

impl WitchCommand {
    /// The minimum authorization level required to execute this command.
    pub fn required_authorization(&self) -> AuthorizationLevel {
        // All commands currently require full authentication.
        // FirstTimeSetup-only commands will be added when the auth
        // subsystem is implemented.
        AuthorizationLevel::AuthRequired
    }
}

// ============================================================================
// Responses
// ============================================================================

/// Typed response to a `WitchQuery`.
///
/// Each variant carries the data for exactly one query type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryResponse {
    Status(WorkStatus),
    ReasoningLevel(ReasoningLevel),
    Bool(bool),
    Usize(usize),
    TransactionSummary(Option<TransactionSummaryData>),
    DecisionKeys(Vec<DecisionKey>),
    DecisionLabel(Option<String>),
    HandledDecisionKinds(Vec<DecisionKeyKind>),
    ExternalFetchProgress(Option<FetchProgress>),
    SchemaDescriptions(Vec<String>),
}

/// Response to a `WitchCommand`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandResponse {
    /// Command executed successfully.
    Ok,
    /// Transaction-related error.
    TransactionError(TransactionError),
}

// ============================================================================
// Errors
// ============================================================================

/// Protocol-level errors (distinct from command-level transaction errors).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProtocolError {
    /// Transaction state violation.
    Transaction(TransactionError),
    /// Session token not recognized or expired.
    InvalidSession,
    /// Insufficient authorization level for the requested operation.
    Unauthorized,
    /// Witch not yet initialized (still starting up).
    NotReady,
    /// Unexpected server-side error.
    Internal(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ProtocolError::Transaction(e) => write!(f, "transaction error: {e}"),
            ProtocolError::InvalidSession => write!(f, "invalid or expired session"),
            ProtocolError::Unauthorized => write!(f, "unauthorized"),
            ProtocolError::NotReady => write!(f, "server not ready"),
            ProtocolError::Internal(msg) => write!(f, "internal error: {msg}"),
        }
    }
}

impl std::error::Error for ProtocolError {}

// ============================================================================
// Wire-safe projections
// ============================================================================

/// Wire-safe projection of transaction state.
///
/// The full `PendingTransaction` contains `WitnessedDecision` which holds
/// `Mutation` variants and a `ConfirmationGesture` — neither serializable
/// nor appropriate for the wire. This struct carries just the summary data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionSummaryData {
    /// Human-readable label for the transaction.
    pub label: String,
    /// Number of decisions staged.
    pub decision_count: usize,
    /// Total mutations across all staged decisions.
    pub mutation_count: usize,
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Round-trip all protocol enum variants through serde_json to verify
    /// Serialize + Deserialize are correctly derived on the full type graph.
    #[test]
    fn protocol_types_serialize() {
        // SessionId
        let sid = SessionId(42);
        let json = serde_json::to_string(&sid).unwrap();
        let rt: SessionId = serde_json::from_str(&json).unwrap();
        assert_eq!(sid, rt);

        // AuthorizationLevel — all variants
        for level in [
            AuthorizationLevel::FirstTimeSetup,
            AuthorizationLevel::Unauthenticated,
            AuthorizationLevel::AuthRequired,
        ] {
            let json = serde_json::to_string(&level).unwrap();
            let rt: AuthorizationLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(level, rt);
        }

        // WitchQuery — representative variants
        let queries = vec![
            WitchQuery::Status,
            WitchQuery::ReasoningLevel,
            WitchQuery::HasPending,
            WitchQuery::IsInitialScanning,
            WitchQuery::DbQueueDepth,
            WitchQuery::HasTransaction,
            WitchQuery::TransactionSummary,
            WitchQuery::DecisionKeys,
            WitchQuery::GetDecision(DecisionKey::Deploy),
            WitchQuery::HandledDecisionKinds,
            WitchQuery::IsExternalFetchActive,
            WitchQuery::ExternalFetchProgress,
            WitchQuery::HasAcoustIdApiKey,
            WitchQuery::NeedsSchemaUpdate,
            WitchQuery::PendingSchemaDescriptions,
        ];
        for q in &queries {
            let json = serde_json::to_string(q).unwrap();
            let _rt: WitchQuery = serde_json::from_str(&json).unwrap();
        }

        // WitchCommand — all variants
        let commands = vec![
            WitchCommand::StartTransaction {
                label: "test".into(),
            },
            WitchCommand::AddDecision {
                key: DecisionKey::Deploy,
                label: "deploy all".into(),
                mutation_count: 5,
            },
            WitchCommand::RemoveDecision {
                key: DecisionKey::OobSync,
            },
            WitchCommand::ConfirmTransaction,
            WitchCommand::DiscardTransaction,
            WitchCommand::RequestExternalFetch,
            WitchCommand::RequestReleasePacking,
            WitchCommand::QueueSchemaReconciliation,
            WitchCommand::QueueVacuum,
            WitchCommand::LatchReadOnlyForSafety {
                reason: "testing".into(),
            },
        ];
        for c in &commands {
            let json = serde_json::to_string(c).unwrap();
            let _rt: WitchCommand = serde_json::from_str(&json).unwrap();
        }

        // QueryResponse — all variants
        let responses = vec![
            QueryResponse::Status(WorkStatus::default()),
            QueryResponse::ReasoningLevel(ReasoningLevel::None),
            QueryResponse::Bool(true),
            QueryResponse::Usize(42),
            QueryResponse::TransactionSummary(Some(TransactionSummaryData {
                label: "test tx".into(),
                decision_count: 3,
                mutation_count: 10,
            })),
            QueryResponse::TransactionSummary(None),
            QueryResponse::DecisionKeys(vec![DecisionKey::Deploy, DecisionKey::OobSync]),
            QueryResponse::DecisionLabel(Some("label".into())),
            QueryResponse::DecisionLabel(None),
            QueryResponse::HandledDecisionKinds(vec![
                DecisionKeyKind::OobSync,
                DecisionKeyKind::MissingFile,
            ]),
            QueryResponse::ExternalFetchProgress(None),
            QueryResponse::SchemaDescriptions(vec!["add column foo".into()]),
        ];
        for r in &responses {
            let json = serde_json::to_string(r).unwrap();
            let _rt: QueryResponse = serde_json::from_str(&json).unwrap();
        }

        // CommandResponse
        let cmd_responses = vec![
            CommandResponse::Ok,
            CommandResponse::TransactionError(TransactionError::AlreadyActive),
            CommandResponse::TransactionError(TransactionError::NoActiveTransaction),
            CommandResponse::TransactionError(TransactionError::NotAcceptingMutations),
            CommandResponse::TransactionError(TransactionError::Unauthorized),
        ];
        for r in &cmd_responses {
            let json = serde_json::to_string(r).unwrap();
            let _rt: CommandResponse = serde_json::from_str(&json).unwrap();
        }

        // ProtocolError — all variants
        let errors = vec![
            ProtocolError::Transaction(TransactionError::AlreadyActive),
            ProtocolError::InvalidSession,
            ProtocolError::Unauthorized,
            ProtocolError::NotReady,
            ProtocolError::Internal("boom".into()),
        ];
        for e in &errors {
            let json = serde_json::to_string(e).unwrap();
            let _rt: ProtocolError = serde_json::from_str(&json).unwrap();
        }

        // WitchQuery authorization levels
        assert_eq!(
            WitchQuery::Status.required_authorization(),
            AuthorizationLevel::AuthRequired,
        );
        assert_eq!(
            WitchQuery::NeedsSchemaUpdate.required_authorization(),
            AuthorizationLevel::Unauthenticated,
        );
        assert_eq!(
            WitchQuery::PendingSchemaDescriptions.required_authorization(),
            AuthorizationLevel::Unauthenticated,
        );
        assert_eq!(
            WitchCommand::RequestExternalFetch.required_authorization(),
            AuthorizationLevel::AuthRequired,
        );

        // TransactionSummaryData
        let summary = TransactionSummaryData {
            label: "deploy batch".into(),
            decision_count: 5,
            mutation_count: 20,
        };
        let json = serde_json::to_string(&summary).unwrap();
        let rt: TransactionSummaryData = serde_json::from_str(&json).unwrap();
        assert_eq!(summary.label, rt.label);
        assert_eq!(summary.decision_count, rt.decision_count);
        assert_eq!(summary.mutation_count, rt.mutation_count);
    }
}
