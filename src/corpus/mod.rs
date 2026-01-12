//! Corpus Module
//!
//! Unified API for corpus health, analysis, and reporting. This module brings
//! together related functionality that was previously scattered across multiple
//! top-level modules.
//!
//! ## Submodules
//!
//! - `health/` - Health issue detection, filtering, and library health
//! - `fingerprint/` - Fingerprint quality analysis and comparison
//! - `reports/` - Report generation with ReportRenderer pattern
//!
//! ## Future Additions
//!
//! - `summary` - Aggregated corpus and health summaries

pub mod fingerprint;
pub mod health;
pub mod reports;

// Re-export commonly used items from health
pub use health::{
    detect_and_store_canonicalizations,
    detect_fingerprint_issues,
    detect_metadata_issues,
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
