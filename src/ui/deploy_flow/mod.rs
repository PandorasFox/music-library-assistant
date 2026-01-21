//! Deployment Flow UI Module
//!
//! Provides the interactive workflow for deploying corpus files to libraries.
//! Shows deployment preview with statistics, then transitions to session review
//! for commit/cancel.

pub mod preview;
pub mod types;

pub use preview::{DeploymentPreviewAction, DeploymentPreviewState};
pub use types::{DeployConfirmModal, DeployModalData, DeploySummary};
