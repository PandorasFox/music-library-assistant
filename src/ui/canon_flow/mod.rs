//! Artist Canonicalization Flow UI Module
//!
//! Provides the interactive workflow for resolving artist name spelling variants.
//! Presents buckets of similar artist names in a two-pane layout, allowing the
//! librarian to select which variants to consolidate and choose the canonical form.
//!
//! ## Flow
//!
//! 1. **Cluster View** - Two-pane layout per cluster:
//!    - Left pane: Variants with checkboxes and track counts
//!    - Right pane: Name field + "Squash!" button
//! 2. **Session Review** - Review all decisions with scrollable list and summary
//! 3. **Commit Modal** - Popup showing completion and background task status
//!
//! ## Key Bindings
//!
//! Cluster View:
//! - Left/Right: Move focus between panes
//! - Up/Down: Navigate within pane (variants list or Name/Squash in action pane)
//! - Space: Toggle selection (variants pane) or toggle squash state (Squash button)
//! - A: Toggle all based on majority (variants pane)
//! - Tab: Advance to next group (preserves decision)
//! - Shift+Tab: Go back to previous group
//! - Esc: Go to session review
//!
//! Session Review:
//! - Up/Down: Scroll through decisions
//! - Enter: Commit all changes
//! - Esc: Cancel and discard

pub mod cluster_view;
pub(crate) mod coordinator;
pub mod review;
pub mod session;

pub use cluster_view::{ClusterViewAction, ClusterViewState};
pub use review::{ReviewAction, ReviewState};
#[allow(unused_imports)]
pub use session::{ArtistBucket, ArtistVariant, CanonDecision, CanonSession};

// TODO: Health check for tags mismatching on-disk vs in-index
// - This flow's pattern (bucket selection -> confirmation -> commit) could be
//   adapted for resolving OutOfBandTagChange health issues
// - Resolution options: flush index to disk OR accept out-of-band changes
// - Granularity TBD (potentially per-directory config)
