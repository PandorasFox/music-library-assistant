//! Insights View Module
//!
//! A full-screen view displaying computed insights over health signals.
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.
//!
//! ## Four-Bucket Structure
//!
//! Insights are organized into four buckets with distinct purposes:
//! 1. **Corpus Files** - OOB changes (top priority), indexed/unindexed/missing counts
//! 2. **Placeholder** - Reserved for future use (displays `:)`)
//! 3. **Library/Deploy** - Stale, leftover, ready-to-deploy, deployed healthy
//! 4. **Other Signals** - Remaining signals sorted by count
//!
//! ## Navigation
//!
//! - Up/Down: Navigate within and between buckets
//! - Enter: Launch flow for selected insight (blocked when Witch is busy)
//! - Tab/Shift-Tab: Cycle to adjacent view
//! - Esc: Return to main menu
//!
//! ## Modal State
//!
//! The view tracks whether the Witch is busy. When busy, actionable
//! insights are dimmed and the Enter key is blocked.

mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::corpus::db::types::InsightsData;
use crate::witch::DaemonStatus;

pub use render::render_insights_view;

/// Action returned from input handling
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InsightsAction {
    /// No action needed
    None,
    /// Request to quit the application (show confirmation)
    RequestQuit,
    /// Cycle to next view in ring
    CycleNext,
    /// Cycle to previous view in ring
    CyclePrev,
    /// Launch flow for selected insight
    LaunchFlow,
}

/// State for the insights view modal/status
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(non_camel_case_types)]
pub enum InsightsModal {
    /// Ready for user interaction
    Ready,
    /// The Witch has operations in-flight - actions blocked
    NotReady_WitchBusy,
}

impl Default for InsightsModal {
    fn default() -> Self {
        Self::Ready
    }
}

/// Which bucket currently has focus for navigation
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusedBucket {
    #[default]
    Corpus,
    Placeholder,
    Library,
    Other,
}

impl FocusedBucket {
    /// Get the index of this bucket (0-3)
    pub fn index(self) -> usize {
        match self {
            FocusedBucket::Corpus => 0,
            FocusedBucket::Placeholder => 1,
            FocusedBucket::Library => 2,
            FocusedBucket::Other => 3,
        }
    }

    /// Get the next bucket in order
    fn next(self) -> Self {
        match self {
            FocusedBucket::Corpus => FocusedBucket::Placeholder,
            FocusedBucket::Placeholder => FocusedBucket::Library,
            FocusedBucket::Library => FocusedBucket::Other,
            FocusedBucket::Other => FocusedBucket::Other, // Stay at end
        }
    }

    /// Get the previous bucket in order
    fn prev(self) -> Self {
        match self {
            FocusedBucket::Corpus => FocusedBucket::Corpus, // Stay at start
            FocusedBucket::Placeholder => FocusedBucket::Corpus,
            FocusedBucket::Library => FocusedBucket::Placeholder,
            FocusedBucket::Other => FocusedBucket::Library,
        }
    }
}

/// Selection state within a single bucket
#[derive(Debug, Clone, Default)]
pub struct BucketSelection {
    /// Index of selected item within this bucket
    pub selected: usize,
    /// Scroll offset for rendering
    pub scroll: usize,
}

/// State for the insights view
pub struct InsightsViewState {
    /// Modal state tracking Witch busy status
    pub modal: InsightsModal,
    /// Which bucket currently has navigation focus
    pub focused_bucket: FocusedBucket,
    /// Selection state for each bucket (indexed by FocusedBucket::index())
    pub bucket_selections: [BucketSelection; 4],
    /// Cached insights data from UiReadCache
    pub cached_data: Option<InsightsData>,
}

impl Default for InsightsViewState {
    fn default() -> Self {
        Self {
            modal: InsightsModal::Ready,
            focused_bucket: FocusedBucket::default(),
            bucket_selections: Default::default(),
            cached_data: None,
        }
    }
}

impl InsightsViewState {
    /// Create a new insights view state
    pub fn new() -> Self {
        Self::default()
    }

    /// Update state every tick - checks Witch status and caches insights data
    pub fn update(&mut self, witch_status: Option<&DaemonStatus>, insights_data: Option<InsightsData>) {
        let busy = witch_status
            .map(|s| s.pending > 0)
            .unwrap_or(false);

        self.modal = if busy {
            InsightsModal::NotReady_WitchBusy
        } else {
            InsightsModal::Ready
        };

        // Update cached data if new data available
        if insights_data.is_some() {
            self.cached_data = insights_data;
        }
    }

    /// Check if the Witch is busy (actions should be blocked)
    pub fn is_witch_busy(&self) -> bool {
        matches!(self.modal, InsightsModal::NotReady_WitchBusy)
    }

    /// Check if a deploy-related insight is currently selected.
    ///
    /// Returns true if the Library bucket is focused (any of: stale, leftover, deploy_ready, deployed_healthy)
    pub fn is_deploy_insight_selected(&self) -> bool {
        self.focused_bucket == FocusedBucket::Library
    }

    /// Get the entry count for a specific bucket
    pub fn get_bucket_entry_count(&self, bucket: FocusedBucket) -> usize {
        match bucket {
            // Corpus bucket: OOB modified, OOB tags, files in corpus, indexed, unindexed, missing, relocated
            FocusedBucket::Corpus => 7,
            // Placeholder bucket: single entry
            FocusedBucket::Placeholder => 1,
            // Library bucket: stale, leftover, deploy ready, deployed healthy
            FocusedBucket::Library => 4,
            // Other bucket: dynamic based on cached data
            FocusedBucket::Other => {
                self.cached_data
                    .as_ref()
                    .map(|d| d.bucket_other.entries.len())
                    .unwrap_or(0)
            }
        }
    }

    /// Get current bucket's selection state
    fn current_selection(&self) -> &BucketSelection {
        &self.bucket_selections[self.focused_bucket.index()]
    }

    /// Get current bucket's selection state mutably
    fn current_selection_mut(&mut self) -> &mut BucketSelection {
        &mut self.bucket_selections[self.focused_bucket.index()]
    }

    /// Navigate up within the current bucket, or move to previous bucket
    fn navigate_up(&mut self) {
        let selection = self.current_selection_mut();

        if selection.selected > 0 {
            // Move up within current bucket
            selection.selected -= 1;
        } else {
            // At top of bucket - try to move to previous bucket
            let prev_bucket = self.focused_bucket.prev();
            if prev_bucket != self.focused_bucket {
                self.focused_bucket = prev_bucket;
                // Position at end of previous bucket
                let prev_count = self.get_bucket_entry_count(prev_bucket);
                self.current_selection_mut().selected = prev_count.saturating_sub(1);
            }
        }

        // Ensure selection is within bounds (in case entry count changed)
        let current_count = self.get_bucket_entry_count(self.focused_bucket);
        let selection = self.current_selection_mut();
        if selection.selected >= current_count && current_count > 0 {
            selection.selected = current_count - 1;
        }
    }

    /// Navigate down within the current bucket, or move to next bucket
    fn navigate_down(&mut self) {
        let entry_count = self.get_bucket_entry_count(self.focused_bucket);
        let selection = self.current_selection_mut();

        if selection.selected + 1 < entry_count {
            // Move down within current bucket
            selection.selected += 1;
        } else {
            // At bottom of bucket - try to move to next bucket
            let next_bucket = self.focused_bucket.next();
            if next_bucket != self.focused_bucket {
                self.focused_bucket = next_bucket;
                // Position at start of next bucket
                self.current_selection_mut().selected = 0;
            }
        }
    }

    /// Navigate to the very first entry (bucket 1, item 0)
    fn navigate_to_start(&mut self) {
        self.focused_bucket = FocusedBucket::Corpus;
        for selection in &mut self.bucket_selections {
            selection.selected = 0;
            selection.scroll = 0;
        }
    }

    /// Navigate to the very last entry (last bucket, last item)
    fn navigate_to_end(&mut self) {
        // Find last non-empty bucket
        for bucket in [FocusedBucket::Other, FocusedBucket::Library, FocusedBucket::Placeholder, FocusedBucket::Corpus] {
            let count = self.get_bucket_entry_count(bucket);
            if count > 0 {
                self.focused_bucket = bucket;
                self.bucket_selections[bucket.index()].selected = count - 1;
                return;
            }
        }
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> InsightsAction {
        match key.code {
            KeyCode::Esc => InsightsAction::RequestQuit,

            KeyCode::Up | KeyCode::Char('k') => {
                self.navigate_up();
                InsightsAction::None
            }

            KeyCode::Down | KeyCode::Char('j') => {
                self.navigate_down();
                InsightsAction::None
            }

            KeyCode::Home => {
                self.navigate_to_start();
                InsightsAction::None
            }

            KeyCode::End => {
                self.navigate_to_end();
                InsightsAction::None
            }

            KeyCode::Enter => {
                // Block launch if the Witch is busy
                if self.is_witch_busy() {
                    return InsightsAction::None;
                }
                // TODO: Launch flow when implemented
                InsightsAction::LaunchFlow
            }

            KeyCode::Tab => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    InsightsAction::CyclePrev
                } else {
                    InsightsAction::CycleNext
                }
            }

            KeyCode::BackTab => InsightsAction::CyclePrev,

            _ => InsightsAction::None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insights_action_exit() {
        let mut state = InsightsViewState::new();
        let action = state.handle_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::RequestQuit);
    }

    #[test]
    fn test_witch_busy_blocks_enter() {
        let mut state = InsightsViewState::new();

        // Not busy - Enter should launch flow
        state.modal = InsightsModal::Ready;
        let action = state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::LaunchFlow);

        // Busy - Enter should be blocked
        state.modal = InsightsModal::NotReady_WitchBusy;
        let action = state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::None);
    }

    #[test]
    fn test_update_witch_status() {
        let mut state = InsightsViewState::new();

        // No status - should be Ready
        state.update(None, None);
        assert_eq!(state.modal, InsightsModal::Ready);

        // Pending > 0 - should be busy
        let busy_status = DaemonStatus {
            pending: 5,
            ..Default::default()
        };
        state.update(Some(&busy_status), None);
        assert_eq!(state.modal, InsightsModal::NotReady_WitchBusy);

        // Pending = 0 - should be ready again
        let idle_status = DaemonStatus {
            pending: 0,
            ..Default::default()
        };
        state.update(Some(&idle_status), None);
        assert_eq!(state.modal, InsightsModal::Ready);
    }

    #[test]
    fn test_tab_navigation_not_blocked() {
        let mut state = InsightsViewState::new();
        state.modal = InsightsModal::NotReady_WitchBusy;

        // Tab should still work even when the Witch is busy
        let action = state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::CycleNext);

        let action = state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(action, InsightsAction::CyclePrev);
    }

    #[test]
    fn test_bucket_navigation() {
        let mut state = InsightsViewState::new();

        // Start at Corpus bucket, item 0
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 0);

        // Navigate down within corpus bucket
        state.navigate_down();
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 1);

        // Navigate to end of corpus bucket (7 items: 0-6)
        for _ in 0..5 {
            state.navigate_down();
        }
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 6);

        // Navigate down should move to Placeholder bucket
        state.navigate_down();
        assert_eq!(state.focused_bucket, FocusedBucket::Placeholder);
        assert_eq!(state.current_selection().selected, 0);

        // Navigate up should return to Corpus bucket at last item
        state.navigate_up();
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 6);
    }

    #[test]
    fn test_navigate_to_start_and_end() {
        let mut state = InsightsViewState::new();

        // Move around a bit
        state.focused_bucket = FocusedBucket::Library;
        state.bucket_selections[FocusedBucket::Library.index()].selected = 2;

        // Navigate to start
        state.navigate_to_start();
        assert_eq!(state.focused_bucket, FocusedBucket::Corpus);
        assert_eq!(state.current_selection().selected, 0);

        // Navigate to end (Library bucket has 4 items, so last is index 3)
        state.navigate_to_end();
        assert_eq!(state.focused_bucket, FocusedBucket::Library);
        assert_eq!(state.current_selection().selected, 3);
    }
}
