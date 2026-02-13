//! Progressive Worker Types
//!
//! Core types for the progressive worker modal: work items, state, and completion handlers.

use std::collections::VecDeque;

use crate::meta::signals::data::CompoundGroup;

/// Unit of work to be processed incrementally.
///
/// Each variant represents a specific type of bulk operation that can be
/// processed in time-sliced chunks on the UI thread.
#[derive(Debug, Clone)]
pub enum WorkItem {
    /// Stage mutations for a compound split group (aggregated by value).
    StageCompoundSplit {
        /// The group of inodes sharing a compound value
        group: CompoundGroup,
        /// Index in the overall work queue (for decision numbering)
        idx: usize,
    },
    // Future: LoadSignalData, LoadDeployPreview, etc.
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
}

impl ProgressiveWorkerState {
    /// Create a new progressive worker state.
    ///
    /// # Arguments
    /// * `label` - Title shown in the progress modal
    /// * `items` - Work items to process
    /// * `on_complete` - Callback identifier for completion handling
    pub fn new(label: String, items: Vec<WorkItem>, on_complete: OnComplete) -> Self {
        let total = items.len();
        Self {
            label,
            work_queue: VecDeque::from(items),
            processed: 0,
            total,
            current_label: None,
            mutations_generated: 0,
            nops_elided: 0,
            on_complete,
            is_safe_mode: false,
        }
    }

    /// Create a new progressive worker for compound splits.
    pub fn for_compound_splits(groups: Vec<CompoundGroup>, is_safe_mode: bool) -> Self {
        let items: Vec<WorkItem> = groups
            .into_iter()
            .enumerate()
            .map(|(idx, group)| WorkItem::StageCompoundSplit { group, idx })
            .collect();

        let mut state = Self::new(
            "Staging compound splits...".to_string(),
            items,
            OnComplete::CompoundSplitStaging,
        );
        state.is_safe_mode = is_safe_mode;
        state
    }

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
