//! Configuration types: structs, enums, Default impls, and Config methods.
//!
//! Type definitions live in mm-meta; re-exported here.
//! Filesystem validation (which touches crate-local state) stays here.

// Re-export all config types from mm-meta
pub use mm_meta::config::{
    read_shared_config, AlbumArtOpinions, CanonicalizationOpinions, Config, CreditRoutingConfig,
    DiscExtractionOpinions, DuplicateAnalysisOpinions, ExternalMatchingConfig,
    HealthDetectionOpinions, MbTagNameConfig, Opinions, PackingWeights, PerformanceOpinions,
    QualityResolutionOpinions, RelationRouting, ReleasePackingOpinions, SharedConfig,
    SidecarDeployMode, SourceDir, StartupOpinions, StartupView, TagSplittingOpinions,
};

// Config validation (filesystem checks, source path containment) previously lived here.
// Removed: config changes now flow through mutations. Filesystem validation needs
// reimplementation as a mutation-triggered check that calls
// corpus::paths::set_expected_device_id().
