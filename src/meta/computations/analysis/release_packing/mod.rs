//! Release bin-packing staged pipeline.
//!
//! See `docs/RELEASE_PACKING_ALGORITHM.md` for the full algorithm reference.
//! Keep that document in sync with any changes to scoring, staging, or classification logic.
//!
//! Multi-stage pipeline for assigning corpus files to MusicBrainz releases:
//!
//! ```text
//! Stage 1: PackReleases (orchestrator)
//!     │  Loads external matches, identifies releases, writes manifest
//!     │  Spawns N ScoreReleaseCandidates
//!     │  Defers ComputeReleaseMappings
//!     ▼
//! Stage 2: ScoreReleaseCandidates { release_id } × N  (parallel)
//!     │  AcoustID Hungarian matching + per-release elimination
//!     │  Each release builds its maximally-packed proposal independently
//!     │  Writes results to intermediate table
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3a: ComputeReleaseMappings (orchestrator)
//!     │  Classifies proposals into quality tiers, defers MIS rounds
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3b: MapPerfectReleases — MIS on 1:1 dir↔release proposals
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 3c: MapFullMatchReleases — MIS on cross-dir/extra-file proposals
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stages 3d/3e: MapIncompleteReleases + MapSingleReleases
//!     │  Order configurable via `singles-before-incompletes`:
//!     │    false (default): Incomplete → Singles
//!     │    true:            Singles → Incomplete
//!     │  ═══════ BARRIER ═══════
//!     ▼
//! Stage 4: EmitUnmatchedSignals
//!        Unmatched corpus tracks, unfilled release slots
//! ```
//!
//! Each tier emits PackedRelease, ReleasePacking, and PackingKnot signals
//! per-component immediately after MIS solving. Inter-tier inode tracking
//! reads assigned inodes from the DB (signal_release_packing table).
//! State flows between MIS rounds via `SharedMappingState` (Arc<Mutex<Option<Box>>>).
//!
//! Manual trigger only — not part of ScheduleContentAnalysis.

mod types;
mod scoring;
mod hungarian;
mod mis;
mod components;
pub(crate) mod pinned;
mod stage_scoring;
mod stage_mapping;
mod stage_unmatched;

// Re-export public types (keeps release_packing::SharedMappingState etc. working)
pub(crate) use types::*;

// Re-export execute_* functions (keeps analysis/mod.rs dispatch working)
pub use stage_scoring::*;
pub use stage_mapping::*;
pub use stage_unmatched::*;
pub(crate) use components::execute_resolve_packing_component;
