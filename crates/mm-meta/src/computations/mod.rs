//! Computation pipeline vocabulary types.
//!
//! Pure data types for the computation system's phase-stratified pipeline.
//! Execution logic stays in mm — these are the enums and structs that
//! need to be visible to both server and client.

pub mod derivation;
pub mod observation;
pub mod recomputation;
pub mod types;

// ============================================================================
// Pipeline Stages (multi-phase computation barriers)
// ============================================================================

/// Named stages for multi-phase computation pipelines.
///
/// Orchestrator computations (e.g., PackReleases) return `deferred_phases`
/// containing barrier-separated follow-up stages. Each stage runs only after
/// all prior work drains (in-flight tasks + db write queue).
///
/// Actual execution order is determined by VecDeque insertion order at the
/// orchestrator, not by variant declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PipelineStage {
    /// Global conflict resolution across scored entities.
    Resolve,
    /// Dependent analysis: spawn computations that read signals written by the
    /// parent computation. General-purpose "write signals → barrier → analyze"
    /// pattern (e.g., DetectFingerprintOverlaps → AnalyzeFingerprintOverlaps).
    DependentAnalysis,
}

impl PipelineStage {
    pub fn label(&self) -> &'static str {
        match self {
            PipelineStage::Resolve => "Resolving",
            PipelineStage::DependentAnalysis => "Dependent analysis",
        }
    }
}
