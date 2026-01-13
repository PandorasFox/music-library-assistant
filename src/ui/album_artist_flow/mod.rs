//! Album Artist Resolution Flow
//!
//! A multi-phase workflow for resolving album_artist tags:
//! - Canonicalization: Resolve spelling/capitalization variants
//! - Collation: Unify mixed-artist albums to "Various Artists"
//! - Population: Bulk-fill missing album_artist tags
//!
//! The flow begins with a phase selector popup where users can
//! choose which phases to run, then proceeds through selected phases.

mod cluster_view;
mod collation;
pub mod coordinator;
mod phase_selector;
mod population;
mod render;
mod review;
mod session;
mod types;

pub use cluster_view::{AlbumArtistClusterAction, AlbumArtistClusterState};
pub use collation::{
    CollationAction, CollationReviewAction, CollationReviewState, CollationSession, CollationState,
};
pub use phase_selector::PhaseSelectorState;
pub use population::{
    PopulationAction, PopulationReviewAction, PopulationReviewState, PopulationSession,
    PopulationState,
};
pub use render::render_phase_selector;
pub use review::{AlbumArtistReviewAction, AlbumArtistReviewState};
pub use session::{AlbumArtistBucket, AlbumArtistCanonSession, AlbumArtistVariant};
pub use types::{AlbumArtistPhase, PhaseSelectorAction};
