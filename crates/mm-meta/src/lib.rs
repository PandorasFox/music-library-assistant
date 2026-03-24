//! mm-meta: Shared protocol types and data structures for Music Magic.
//!
//! This crate contains all types that cross the client/server protocol boundary:
//! - Domain data types (Zone, AudioFile, TagSet, etc.)
//! - Mutation struct definitions (data only, no execution logic)
//! - Decision and transaction types
//! - Signal data types
//! - Protocol message enums and traits
//! - Wire framing helpers
//! - Wire protocol types and framing
//! - Config types
//! - View types (InsightsData, ExternalMatchesData, etc.)

/// MM release version string (shown in title bar and reports).
pub const MM_VERSION: &str = "beta 10";

/// Full application title with version.
pub const MM_TITLE: &str = "Music Magic (mm beta 10)";

pub mod auth;
pub mod computations;
pub mod config;
pub mod db_types;
pub mod decisions;
pub mod domain_queries;
pub mod domain_query_types;
pub mod external;
pub mod logging;
pub mod mutations;
pub mod paths;
pub mod tags;
pub mod signals;
pub mod transcode;
pub mod views;
pub mod witch_types;
pub mod protocol;
pub mod wire;
pub mod client;
