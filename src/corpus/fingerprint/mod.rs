//! Fingerprint Analysis Module
//!
//! Functions for analyzing and comparing audio fingerprints, including:
//! - Filter functions to distinguish duplicates from legitimate variants
//! - Quality-based resolution for auto-resolving duplicates
//!
//! ## Filter Functions
//!
//! Used to detect false positives in fingerprint matching:
//! - `is_same_album_different_tracks`: Detects different songs with matching fingerprints
//! - `durations_within_tolerance`: Filters out tracks with different lengths
//! - `is_legitimate_rerelease`: Identifies legitimate re-releases
//!
//! ## Quality Resolution
//!
//! Used to auto-resolve duplicates when there's a clear quality winner:
//! - `compare_track_quality`: Compare tracks by format tier and bitrate
//! - `auto_resolve_by_quality`: Find auto-resolvable duplicate groups
//! - `categorize_duplicates_by_quality`: Categorize groups for triage

pub mod quality;

// Re-export quality functions (for future use when deduplication.rs is updated)
#[allow(unused_imports)]
pub use quality::{
    auto_resolve_by_quality, categorize_duplicates_by_quality, compare_track_quality,
    AutoResolutionStats, QualityVerdict,
};

// Filter functions are in health/filter.rs and re-exported through corpus::health
// They can be accessed via crate::corpus::health::filter::* or crate::corpus::*
