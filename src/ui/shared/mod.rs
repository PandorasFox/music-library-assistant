//! Shared UI Components
//!
//! Reusable components that can be integrated across multiple flows:
//! - Quality resolution: Bitrate/filetype-based stash options

pub mod quality_resolution;

pub use quality_resolution::{
    analyze_quality, generate_stash_changes, QualityAnalysis, QualityResolutionState,
};
