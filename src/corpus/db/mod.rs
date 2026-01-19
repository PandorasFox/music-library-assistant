//! Central Database Module
//!
//! Core of MLA's toolkit architecture. Common data layer for all operations.
//!
//! This module is organized into:
//! - `types`: Core data structures (Track, ScanStateEntry, DeploymentStats, Health types)
//! - `queries`: All database operations

pub mod queries;
pub mod types;

// Re-export core types
pub use queries::Database;
pub use types::{ScanStateEntry, Track};
pub use types::{
    HealthIssue, HealthIssueSeverity, HealthIssueType, KnownVariant,
    ResolutionType, TrackRole, VariantType,
};
