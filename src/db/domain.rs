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
//! ## Macro
//!
//! The `define_domain_query!` macro generates the query struct, `DomainQuery` impl,
//! and optional `CachedQuery` impl from a compact declaration. Two forms:
//!
//! ```ignore
//! // Simple: single db method call, unwrap_or_default
//! define_domain_query! {
//!     /// Doc comment
//!     GetFoo => FooData, cached(15), db.get_foo_data()
//! }
//!
//! // Body: custom execute logic with `db` in scope
//! define_domain_query! {
//!     /// Doc comment
//!     GetBar => BarData, cached(30), |db| {
//!         let x = db.get_x().unwrap_or_default();
//!         let y = db.get_y().unwrap_or_default();
//!         BarData { x, y }
//!     }
//! }
//!
//! // Uncached (detail query):
//! define_domain_query! {
//!     /// Doc comment
//!     GetBaz => BazData, uncached, db.get_baz_data()
//! }
//! ```
//!
//! See `docs/DOMAIN_QUERY_STRATEGY.md` for the full design rationale.

use std::time::Duration;

use serde::Serialize;

use crate::db::ReadOnlyDb;

// ============================================================================
// Core Traits
// ============================================================================

/// Every domain query implements this. The trait is the contract
/// that the `define_domain_query!` macro generates against.
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
// Macro
// ============================================================================

/// Generates a domain query struct with `DomainQuery` and optional `CachedQuery` impls.
///
/// See module docs for usage examples.
macro_rules! define_domain_query {
    // Simple form: unit struct, single db method, unwrap_or_default
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, cached($secs:expr), db.$method:ident()
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
                db.$method().unwrap_or_default()
            }
        }

        impl CachedQuery for $name {
            const THROTTLE: Duration = Duration::from_secs($secs);
        }
    };

    // Body form: unit struct, custom execute expression with db closure
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, cached($secs:expr), |$db:ident| $body:block
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                $body
            }
        }

        impl CachedQuery for $name {
            const THROTTLE: Duration = Duration::from_secs($secs);
        }
    };

    // Simple form, uncached (detail query)
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, uncached, db.$method:ident()
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, db: &ReadOnlyDb<'_>) -> Self::Response {
                db.$method().unwrap_or_default()
            }
        }
    };

    // Body form, uncached (detail query)
    (
        $( #[doc = $doc:expr] )*
        $name:ident => $response:ty, uncached, |$db:ident| $body:block
    ) => {
        $( #[doc = $doc] )*
        #[derive(serde::Serialize, serde::Deserialize)]
        pub struct $name;

        impl DomainQuery for $name {
            type Response = $response;

            fn execute(self, $db: &ReadOnlyDb<'_>) -> Self::Response {
                $body
            }
        }
    };
}

// ============================================================================
// Summary Queries (CacheReady variants)
// ============================================================================

use crate::meta::views::{
    DeployStatus, EditHistoryData, ExternalMatchesData, InboxOverviewData, InsightsData,
};
use crate::witch::cache_thread::PackingDirsData;

define_domain_query! {
    /// Corpus health insights: file state, tag squash, and other signal counts.
    GetInsights => InsightsData, cached(30), db.get_insights_data()
}

define_domain_query! {
    /// Inbox file counts by category (unindexed, corpus match, organizable, etc.).
    GetInboxOverview => InboxOverviewData, cached(15), db.get_inbox_overview_data()
}

define_domain_query! {
    /// Current deploy status: library health and per-library file counts.
    GetDeployStatus => DeployStatus, cached(15), db.get_deploy_status()
}

define_domain_query! {
    /// Edit session list with timestamps and edit counts.
    GetEditHistory => EditHistoryData, cached(30), |db| {
        let sessions = db.get_edit_sessions().unwrap_or_default();
        EditHistoryData { sessions }
    }
}

define_domain_query! {
    /// External match data: confidence buckets, packing counts, untagged entries.
    GetExternalMatches => ExternalMatchesData, cached(15), db.get_external_matches_data()
}

define_domain_query! {
    /// Packing directory data: assigned file paths and directory categories.
    GetPackingDirs => PackingDirsData, cached(30), |db| {
        let file_paths = db.get_packing_assigned_paths().unwrap_or_default();
        let dir_categories = db.get_packing_directory_categories().unwrap_or_default();
        PackingDirsData { file_paths, dir_categories }
    }
}
