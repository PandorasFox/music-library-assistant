//! Domain Query Layer
//!
//! Typed query vocabulary for all read access to MM's database. Any client
//! (TUI, web API, future tooling) communicates with the Witch through these
//! query types rather than raw `ReadOnlyDb` handles.
//!
//! ## Trait Contract
//!
//! Every query implements `DomainQuery` with exactly this shape:
//! - All inputs are fields on the query struct
//! - All outputs are in the associated `Response` type
//! - `execute` takes `self` by value and `&ReadOnlyDb` — nothing else
//! - Response types are `Serialize` (web-ready from day one)
//! - Errors are handled internally (return usable defaults, never `Result`)
//!
//! This uniformity is intentional. A future proc-macro will generate the
//! boilerplate (query struct, response struct, dispatch, serde) from
//! annotated functions — but only if every hand-written query follows
//! the trait contract exactly.
//!
//! See `docs/DOMAIN_QUERY_STRATEGY.md` for the full design rationale.

use std::time::Duration;

use serde::Serialize;

use crate::db::ReadOnlyDb;
use crate::meta::views::DeployStatus;

// ============================================================================
// Core Traits
// ============================================================================

/// Every domain query implements this. The trait is the contract
/// that a future proc-macro will generate against.
///
/// Implementors must follow the uniformity rules:
/// 1. All inputs are fields on `self`
/// 2. All outputs are in `Response`
/// 3. No side channels, no `&mut`, no extra context parameters
/// 4. Errors handled internally — return defaults, never `Result`
pub trait DomainQuery: Send + 'static {
    /// The response type. Must be `Serialize` for web transport readiness.
    type Response: Serialize + Send + 'static;

    /// Execute the query against a read-only database connection.
    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response;
}

/// Summary queries that benefit from throttled caching.
/// Detail queries implement only `DomainQuery`.
pub trait CachedQuery: DomainQuery {
    /// How long cached results remain fresh before re-query.
    const THROTTLE: Duration;
}

// ============================================================================
// Query: GetDeployStatus
// ============================================================================

/// Current deploy status: library health and per-library file counts.
///
/// Summary query — cheap, always requested (titlebar uses it every frame).
#[derive(serde::Serialize, serde::Deserialize)]
pub struct GetDeployStatus;

impl DomainQuery for GetDeployStatus {
    type Response = DeployStatus;

    fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
        db.get_deploy_status().unwrap_or_default()
    }
}

impl CachedQuery for GetDeployStatus {
    const THROTTLE: Duration = Duration::from_secs(15);
}
