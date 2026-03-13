//! Deploy signal data types.

use serde::{Deserialize, Serialize};

/// Serializable metadata for a sidecar deploy-ready signal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SidecarDeployReadyData {
    pub role: String,
    pub format: String,
    pub width: u32,
    pub height: u32,
}

/// Deploy lifecycle phases for corpus and library files (audio and sidecar).
///
/// Used on both sides of the deploy pipeline:
/// - **Library side** (`DeriveDeployHealthSignals`): classifies each library
///   file as Healthy, Stale, or Leftover.
/// - **Corpus side** (`DeriveCorpusDeployStatus`): classifies each healthy
///   corpus file as Ready, Healthy (deployed correctly), or Stale (deployed
///   at wrong path). Conflict/overlap filters then suppress Ready signals.
///
/// Exhaustive match on this enum at classification sites ensures
/// both audio and sidecar codepaths handle all phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeployLifecyclePhase {
    /// Corpus file ready for deployment (not yet in any library).
    Ready,
    /// File correctly deployed at expected library path.
    Healthy,
    /// File deployed but at wrong path (tags changed since deploy).
    Stale,
    /// Library file with no corpus backing (orphan).
    Leftover,
}
