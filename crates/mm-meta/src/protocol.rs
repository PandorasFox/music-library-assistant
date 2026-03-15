//! Protocol types for the Witch client-server architecture.
//!
//! The protocol defines the complete vocabulary for UI<>Witch communication.
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
//! the `domain_query_protocol!` macro in `domain_queries`. Adding a new query
//! = adding one line to the macro invocation.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::decisions::{Decision, DecisionKey, DiscardSummary, TransactionError};
use crate::auth::SessionToken;
use crate::config::Config;
use crate::witch_types::WitchStatus;

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnauthenticatedBody {
    /// Attempt login with credentials.
    Login { username: String, password: String },
    /// Complete first-time setup with archive root and optional first user.
    CompleteSetup {
        root: PathBuf,
        first_user: Option<(String, String)>,
    },
    /// Check whether first-time setup is needed.
    SetupQuery,
}

/// Response to unauthenticated requests.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum UnauthenticatedResponse {
    /// Login result.
    Auth(AuthResponse),
    /// Setup completed successfully.
    SetupComplete,
    /// Setup status response.
    SetupStatus {
        needs_setup: bool,
        /// Suggested default root from MM_ROOT env var, if set.
        suggested_root: Option<PathBuf>,
    },
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthenticatedBody {
    Query(QueryPayload),
    Transaction(TransactionPayload),
    Command(Box<CommandPayload>),
}

/// Response to authenticated requests.
///
/// Mirrors the three sub-areas. Server dispatch returns the variant
/// matching the request area.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AuthenticatedResponse {
    Query(Box<QueryResponse>),
    Transaction(TransactionResponse),
    Command(CommandResponse),
}

// ============================================================================
// Query Area
// ============================================================================

/// Read-only query payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryPayload {
    /// Comprehensive Witch state snapshot.
    Status,
    /// Current config snapshot.
    Config,
    /// Raw KDL text of config.kdl (for source detection in config editor).
    ConfigKdl,
    /// Domain-specific DB query (dispatched to cache thread).
    Domain(Box<crate::domain_queries::DomainQueryPayload>),
}

/// Query response variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryResponse {
    /// Full Witch status snapshot.
    Status(WitchStatus),
    /// Config snapshot.
    Config(Box<Config>),
    /// Raw KDL text from config.kdl.
    ConfigKdl(String),
    /// Domain query result.
    Domain(crate::domain_queries::DomainQueryResult),
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

/// Query for the current config snapshot.
pub struct ConfigQuery;

impl ProtocolQuery for ConfigQuery {
    type Response = Config;

    fn into_payload(self) -> QueryPayload {
        QueryPayload::Config
    }

    fn extract_response(resp: QueryResponse) -> Config {
        match resp {
            QueryResponse::Config(c) => *c,
            _ => unreachable!("protocol bug: expected Config response"),
        }
    }
}

/// Query for the raw KDL text of config.kdl.
pub struct ConfigKdlQuery;

impl ProtocolQuery for ConfigKdlQuery {
    type Response = String;

    fn into_payload(self) -> QueryPayload {
        QueryPayload::ConfigKdl
    }

    fn extract_response(resp: QueryResponse) -> String {
        match resp {
            QueryResponse::ConfigKdl(s) => s,
            _ => unreachable!("protocol bug: expected ConfigKdl response"),
        }
    }
}

// ============================================================================
// Transaction Area
// ============================================================================

/// Transaction lifecycle operations.
///
/// Transactions are their own authenticated sub-area with typed responses.
/// All variants are serializable — `Decision` is the protocol-level unit
/// of operator intent (label + mutations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum TransactionPayload {
    /// Open a new transaction.
    Start { label: String },
    /// Stage a decision in the active transaction.
    AddDecision {
        key: DecisionKey,
        decision: Decision,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecisionDetail {
    pub key: DecisionKey,
    pub label: String,
    pub mutations: Vec<crate::mutations::Mutation>,
}

// ============================================================================
// Command Area
// ============================================================================

/// Background task types for `CommandPayload::QueueTask`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BackgroundTask {
    /// AcoustID + MusicBrainz external metadata fetch.
    ExternalFetch,
    /// Release bin-packing computation.
    ReleasePacking,
    /// Schema reconciliation pass.
    SchemaReconciliation,
    /// SQLite VACUUM.
    Vacuum,
}

/// Operational commands with no transaction semantics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandPayload {
    /// Queue a background task.
    QueueTask(BackgroundTask),
    /// Initiate server shutdown.
    Shutdown,
}

/// Command response variants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CommandResponse {
    /// Command executed successfully.
    Ok,
    /// Command received but could not execute (configuration issue, etc).
    Failed(String),
    /// Server is shutting down. This is the last response the client will receive.
    Goodbye,
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
