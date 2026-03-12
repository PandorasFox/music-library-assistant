//! Protocol types for the Witch client-server architecture.
//!
//! The protocol defines the complete vocabulary for UI↔Witch communication.
//! Two top-level request types (unauthenticated, authenticated) with typed
//! sub-areas for queries, transactions, and commands.
//!
//! ## Trait-Based Request/Response Coupling
//!
//! `ProtocolQuery` and `ProtocolCommand` traits have associated `Response` types.
//! Client-side send methods are generic over these traits, so callsites get
//! typed responses without manual variant matching.
//!
//! ## Domain Queries
//!
//! Domain queries (reads against the DB) are lifted into the protocol via
//! the `domain_query_protocol!` macro in `db/domain.rs`. Adding a new query
//! = adding one line to the macro invocation.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::decisions::{DecisionKey, TransactionError};
use crate::auth::SessionToken;
use crate::meta::decisions::{DiscardSummary, WitnessedDecision};
use crate::witch::WitchStatus;

// ============================================================================
// Authorization
// ============================================================================

/// Authorization level for the Witch's gate.
///
/// Two system states, two levels. Exact-match, fail-closed semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AuthorizationLevel {
    /// No users exist yet. Only `CompleteSetup` accepted.
    FirstTimeSetup,
    /// Valid session token. All operational endpoints.
    Authenticated,
}

// ============================================================================
// Unauthenticated Area (pre-gate — no session token required)
// ============================================================================

/// Request body for unauthenticated operations.
///
/// Login and first-time setup. These bypass the auth gate entirely.
#[derive(Debug, Clone)]
pub enum UnauthenticatedBody {
    /// Attempt login with credentials.
    Login { username: String, password: String },
    /// Complete first-time setup with archive root and optional first user.
    CompleteSetup {
        root: PathBuf,
        first_user: Option<(String, String)>,
    },
}

/// Response to unauthenticated requests.
#[derive(Debug, Clone)]
pub enum UnauthenticatedResponse {
    /// Login result.
    Auth(AuthResponse),
    /// Setup completed successfully.
    SetupComplete,
}

/// Auth response (login result).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthResponse {
    /// Login succeeded — here's the token.
    Token(SessionToken),
    /// Login failed.
    Failed(String),
}

// ============================================================================
// Authenticated Area
// ============================================================================

/// Request body for authenticated operations.
///
/// Three sub-areas: queries (read-only), transactions (decision lifecycle),
/// and commands (operational actions).
pub enum AuthenticatedBody {
    Query(QueryPayload),
    Transaction(TransactionPayload),
    Command(CommandPayload),
}

/// Response to authenticated requests.
///
/// Mirrors the three sub-areas. Server dispatch returns the variant
/// matching the request area.
pub enum AuthenticatedResponse {
    Query(QueryResponse),
    Transaction(TransactionResponse),
    Command(CommandResponse),
}

// ============================================================================
// Query Area
// ============================================================================

/// Read-only query payloads.
pub enum QueryPayload {
    /// Comprehensive Witch state snapshot.
    Status,
    /// Domain-specific DB query (dispatched to cache thread).
    Domain(crate::db::domain::DomainQueryPayload),
}

/// Query response variants.
pub enum QueryResponse {
    /// Full Witch status snapshot.
    Status(WitchStatus),
    /// Domain query result.
    Domain(crate::db::domain::DomainQueryResult),
}

/// Trait for protocol queries. Each implementor declares its response type.
///
/// The wire enums (`QueryPayload`, `QueryResponse`) exist for dispatch routing.
/// Client-side send methods are generic over this trait, so callsites get
/// typed responses: `let status: WitchStatus = handle.send_query(StatusQuery);`
pub trait ProtocolQuery: Send + 'static {
    type Response: Send + 'static;

    /// Pack self into the wire enum.
    fn into_payload(self) -> QueryPayload;

    /// Extract typed response from the wire enum.
    /// Panics on mismatch (indicates protocol bug, not runtime error).
    fn extract_response(resp: QueryResponse) -> Self::Response;
}

/// Query for the comprehensive Witch state snapshot.
pub struct StatusQuery;

impl ProtocolQuery for StatusQuery {
    type Response = WitchStatus;

    fn into_payload(self) -> QueryPayload {
        QueryPayload::Status
    }

    fn extract_response(resp: QueryResponse) -> WitchStatus {
        match resp {
            QueryResponse::Status(s) => s,
            _ => unreachable!("protocol bug: expected Status response"),
        }
    }
}

// ============================================================================
// Transaction Area
// ============================================================================

/// Transaction lifecycle operations.
///
/// Transactions are their own authenticated sub-area with typed responses.
/// `AddDecision` carries non-serializable `WitnessedDecision` data — this
/// is a known gap deferred to mutation serialization work.
#[derive(Debug, Clone)]
pub enum TransactionPayload {
    /// Open a new transaction.
    Start { label: String },
    /// Stage a decision in the active transaction.
    /// NOTE: WitnessedDecision is not serializable. This variant is
    /// enriched in-process only. Deferred to mutation serialization work.
    AddDecision {
        key: DecisionKey,
        decision: WitnessedDecision,
    },
    /// Remove a staged decision.
    RemoveDecision { key: DecisionKey },
    /// Commit the active transaction (execute all staged mutations).
    Confirm,
    /// Discard the active transaction without executing.
    Discard,
    /// Get full details for all decisions in the active transaction.
    GetDetails,
}

/// Transaction operation response.
#[derive(Debug, Clone)]
pub enum TransactionResponse {
    /// Operation succeeded.
    Ok,
    /// Decision details for the active transaction.
    Details(Vec<DecisionDetail>),
    /// Transaction was discarded.
    Discarded(DiscardSummary),
    /// Transaction-specific error.
    Error(TransactionError),
}

/// Detail record for a single decision in the active transaction.
///
/// Heavier than what's in `TransactionSnapshot` — includes mutation data
/// needed for the transaction review view's diff display.
#[derive(Debug, Clone)]
pub struct DecisionDetail {
    pub key: DecisionKey,
    pub label: String,
    pub mutations: Vec<crate::meta::mutations::Mutation>,
}

// ============================================================================
// Command Area
// ============================================================================

/// Operational commands with no transaction semantics.
#[derive(Debug, Clone)]
pub enum CommandPayload {
    /// Kick off external fetch (AcoustID + MusicBrainz lookups).
    RequestExternalFetch,
    /// Kick off release bin-packing computation.
    RequestReleasePacking,
    /// Validate a config blob (returns Ok or error).
    ValidateConfig { config: crate::config::Config },
    /// Inject shared config (called during startup).
    SetSharedConfig { shared: crate::config::SharedConfig },
    /// Start the filesystem watcher.
    StartWatching,
    /// Update performance config at runtime.
    UpdatePerformance {
        opinions: crate::config::PerformanceOpinions,
    },
    /// Queue schema reconciliation.
    QueueSchemaReconciliation,
    /// Queue a VACUUM operation.
    QueueVacuum,
}

/// Command response variants.
#[derive(Debug, Clone)]
pub enum CommandResponse {
    /// Command executed successfully.
    Ok,
    /// Filesystem watcher started (bool = whether watching actually began).
    WatchingStarted(bool),
}

/// Trait for protocol commands. Each implementor declares its response type.
pub trait ProtocolCommand: Send + 'static {
    type Response: Send + 'static;

    /// Pack self into the wire enum.
    fn into_payload(self) -> CommandPayload;

    /// Extract typed response from the wire enum.
    fn extract_response(resp: CommandResponse) -> Self::Response;
}

// ============================================================================
// Errors
// ============================================================================

/// Protocol-level errors (distinct from transaction-level errors).
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

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_query_roundtrip() {
        let q = StatusQuery;
        let payload = q.into_payload();
        assert!(matches!(payload, QueryPayload::Status));

        let response = QueryResponse::Status(WitchStatus::default());
        let result = StatusQuery::extract_response(response);
        assert_eq!(result.mutations_generation, 0);
    }

    #[test]
    fn protocol_error_display() {
        let errors = vec![
            ProtocolError::Transaction(TransactionError::AlreadyActive),
            ProtocolError::InvalidSession,
            ProtocolError::Unauthorized,
            ProtocolError::NotReady,
            ProtocolError::Internal("boom".into()),
        ];
        for e in &errors {
            let s = format!("{e}");
            assert!(!s.is_empty());
        }
    }

    #[test]
    fn authorization_levels_are_distinct() {
        assert_ne!(
            AuthorizationLevel::FirstTimeSetup,
            AuthorizationLevel::Authenticated
        );
    }
}
