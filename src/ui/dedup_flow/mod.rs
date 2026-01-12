//! Deduplication Flow UI Module
//!
//! Provides the interactive workflow for fingerprint-based deduplication.
//! Presents directory set clusters ordered by magnitude, allowing the librarian
//! to select which directory to keep for each cluster.

pub mod bulk_prompt;
pub mod cluster_dialogue;
pub mod session_review;

pub use bulk_prompt::{BulkPromptAction, BulkPromptState};
pub use cluster_dialogue::{ClusterDialogueAction, ClusterDialogueState};
pub use session_review::{SessionReviewAction, SessionReviewState};
