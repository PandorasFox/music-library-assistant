//! Progressive Worker Types
//!
//! Core types for the progressive worker modal: work items, state, and completion handlers.

use std::collections::VecDeque;

use crate::action_handlers::witness::ConfirmationGesture;

/// Unit of work to be processed incrementally.
///
/// Each variant represents a specific type of bulk operation that can be
/// processed in time-sliced chunks on the UI thread.
#[derive(Debug, Clone)]
pub enum WorkItem {
    // Currently no active work item types. The compound split progressive
    // worker was replaced by the V3 interactive modal. Future progressive
    // work types (e.g. batch deploy, bulk tag ops) can be added here.
}

/// Summary of work completed - returned via callback.
#[derive(Debug, Default)]
pub struct WorkSummary {
    /// Items that generated mutations
    pub mutations_generated: usize,
    /// Items skipped (NOP - no changes needed)
    pub nops_elided: usize,
}

/// Progress state for the worker modal.
pub struct ProgressiveWorkerState {
    /// Label shown in title bar (e.g., "Staging compound splits...")
    pub label: String,
    /// Items remaining to process
    pub work_queue: VecDeque<WorkItem>,
    /// Items processed so far
    pub processed: usize,
    /// Total items (for progress bar)
    pub total: usize,
    /// Current item being processed (for display)
    pub current_label: Option<String>,
    /// Tracking for summary
    pub mutations_generated: usize,
    /// Items skipped (no mutations needed)
    pub nops_elided: usize,
    /// What to do when complete
    pub on_complete: OnComplete,
    /// Whether we're in safe mode (for compound splits)
    pub is_safe_mode: bool,
    /// The confirmation gesture that authorized this progressive work.
    pub(crate) _gesture: ConfirmationGesture,
}

impl ProgressiveWorkerState {
    /// Progress ratio for the gauge (0.0 to 1.0).
    pub fn progress_ratio(&self) -> f64 {
        if self.total == 0 {
            1.0
        } else {
            self.processed as f64 / self.total as f64
        }
    }

    /// Build the final work summary.
    pub fn build_summary(&self) -> WorkSummary {
        WorkSummary {
            mutations_generated: self.mutations_generated,
            nops_elided: self.nops_elided,
        }
    }
}

/// Identifies what callback to invoke with WorkSummary on completion.
#[derive(Debug, Clone)]
pub enum OnComplete {
    /// Show transaction review for compound split staging.
    CompoundSplitStaging,
    // Future: LoadDeployPreview, LoadMissingFiles, etc.
}
