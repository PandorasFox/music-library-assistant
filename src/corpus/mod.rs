//! Corpus Module
//!
//! Unified API for corpus indexing, health, analysis, and reporting. This module
//! is the core domain for understanding and maintaining the audio corpus.
//!
//! ## Submodules
//!
//! - `db/` - Database layer (types, queries, changes)
//! - `deduplication/` - Fingerprint-based duplicate detection and resolution
//! - `health/` - Health issue detection, filtering, and library health
//! - `mutations/` - Standardized mutation interface for all corpus changes
//! - `reports/` - Report generation with ReportRenderer pattern

pub mod db;
pub mod deduplication;
pub mod health;
pub mod metadata;
pub mod mutations;
pub mod reports;


// Re-export commonly used items from health
pub use health::{
    detect_and_store_canonicalizations,
    detect_fingerprint_issues,
    spawn_heartbeat,
    HeartbeatResult,
};

// Re-export library health items (for future use)
#[allow(unused_imports)]
pub use health::library::{
    check_all_libraries_health, check_library_health, generate_orphan_cleanup_mutations,
    LibraryHealthResult, OrphanFile, StaleDeployment,
};

// Re-export filter functions for fingerprint analysis (for future use)
#[allow(unused_imports)]
pub use health::filter::{
    durations_within_tolerance, is_legitimate_rerelease, is_same_album_different_tracks,
};

