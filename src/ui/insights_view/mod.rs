//! Insights View Module
//!
//! A full-screen view displaying computed insights over health signals.
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.
//!
//! ## Features
//!
//! - Displays one-dim insights immediately (computed synchronously)
//! - Shows progress for multi-dim insights (computed in background)
//! - Multi-dim insights lift to top when ready (most actionable)
//! - Corpus health shown at bottom (reassuring info)
//! - Eye panel persistent on left side
//!
//! ## Navigation
//!
//! - Up/Down: Navigate insight list
//! - Enter: Launch flow for selected insight (stub - no-op)
//! - Tab/Shift-Tab: Cycle to adjacent view (stub - no-op)
//! - Esc: Return to main menu

mod render;

use std::sync::mpsc;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::corpus::db::Database;
use crate::corpus::health::insights::{
    compute_one_dim_insights, spawn_multi_dim_insight, Insight, MultiDimInsightType,
};
use crate::corpus::health::HeartbeatResult;
use crate::ops::operation::{OperationHandle, OperationMessage};
use crate::ui::widgets::SelectableListState;

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
    /// Launch flow for selected insight (stub)
    LaunchFlow,
}

/// State for the insights view
pub struct InsightsViewState {
    /// Computed insights, sorted by priority (high first)
    pub insights: Vec<Insight>,
    /// Selection state for the list
    pub list_state: SelectableListState,
    /// Handle for in-progress multi-dim computation
    pending_computation: Option<(MultiDimInsightType, OperationHandle)>,
    /// Most recent heartbeat result
    heartbeat: Option<HeartbeatResult>,
    /// Whether initial computation is complete
    initialized: bool,
}

impl Default for InsightsViewState {
    fn default() -> Self {
        Self {
            insights: Vec::new(),
            list_state: SelectableListState::new(),
            pending_computation: None,
            heartbeat: None,
            initialized: false,
        }
    }
}

impl InsightsViewState {
    /// Create a new insights view state
    pub fn new() -> Self {
        Self::default()
    }

    /// Initialize the view with fresh data.
    ///
    /// Called when entering the insights view:
    /// 1. Computes one-dim insights immediately
    /// 2. Spawns background computation for multi-dim insights
    /// 3. Adds "Computing..." placeholder for multi-dim
    pub fn initialize(&mut self, db: &Database, db_path: &str, heartbeat: HeartbeatResult) {
        self.heartbeat = Some(heartbeat.clone());

        // Compute one-dim insights immediately
        let mut insights = compute_one_dim_insights(db, &heartbeat);

        // Add placeholder for multi-dim computation
        insights.push(Insight::Computing {
            insight_type: MultiDimInsightType::QualityDuplicates,
            progress: None,
        });

        // Sort by priority (descending)
        insights.sort_by(|a, b| b.priority().cmp(&a.priority()));

        self.insights = insights;
        self.list_state = SelectableListState::new()
            .with_selected(Some(0))
            .focused();
        self.list_state.total_items = self.insights.len();

        // Spawn multi-dim computation
        let handle = spawn_multi_dim_insight(db_path, MultiDimInsightType::QualityDuplicates);
        self.pending_computation = Some((MultiDimInsightType::QualityDuplicates, handle));

        self.initialized = true;
    }

    /// Update state - poll for background computation progress
    pub fn update(&mut self) {
        // First, collect any pending messages
        let messages: Vec<_> = if let Some((_, ref handle)) = self.pending_computation {
            let mut msgs = Vec::new();
            while let Some(msg) = handle.try_recv() {
                msgs.push(msg);
            }
            msgs
        } else {
            return;
        };

        // Get insight type outside the borrow
        let insight_type = match &self.pending_computation {
            Some((it, _)) => *it,
            None => return,
        };

        // Process messages
        for msg in messages {
            match msg {
                OperationMessage::Progress(progress) => {
                    // Update the Computing placeholder with progress
                    let pct = if progress.total_items > 0 {
                        Some(progress.completed_items as f32 / progress.total_items as f32)
                    } else {
                        None
                    };
                    self.update_computing_progress(insight_type, pct);
                }
                OperationMessage::Complete(result) => {
                    // Replace Computing placeholder with actual insight
                    self.replace_computing_with_result(insight_type, &result);
                    self.pending_computation = None;
                    break;
                }
                OperationMessage::Error(_err) => {
                    // Remove Computing placeholder on error
                    self.remove_computing(insight_type);
                    self.pending_computation = None;
                    break;
                }
                OperationMessage::Cancelled => {
                    self.remove_computing(insight_type);
                    self.pending_computation = None;
                    break;
                }
            }
        }
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> InsightsAction {
        match key.code {
            KeyCode::Esc => InsightsAction::RequestQuit,

            KeyCode::Up | KeyCode::Char('k') => {
                self.list_state.select_previous();
                InsightsAction::None
            }

            KeyCode::Down | KeyCode::Char('j') => {
                self.list_state.select_next();
                InsightsAction::None
            }

            KeyCode::Home => {
                if !self.insights.is_empty() {
                    self.list_state.select(Some(0));
                }
                InsightsAction::None
            }

            KeyCode::End => {
                if !self.insights.is_empty() {
                    self.list_state.select(Some(self.insights.len() - 1));
                }
                InsightsAction::None
            }

            KeyCode::Enter => {
                // Check if selected insight is actionable
                if let Some(insight) = self.selected_insight() {
                    if insight.is_actionable() {
                        InsightsAction::LaunchFlow
                    } else {
                        InsightsAction::None
                    }
                } else {
                    InsightsAction::None
                }
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

    /// Get the currently selected insight
    pub fn selected_insight(&self) -> Option<&Insight> {
        self.list_state.selected().and_then(|i| self.insights.get(i))
    }

    /// Update progress for a Computing placeholder
    fn update_computing_progress(&mut self, insight_type: MultiDimInsightType, progress: Option<f32>) {
        for insight in &mut self.insights {
            if let Insight::Computing { insight_type: it, progress: ref mut p } = insight {
                if *it == insight_type {
                    *p = progress;
                    return;
                }
            }
        }
    }

    /// Replace Computing placeholder with actual result
    fn replace_computing_with_result(
        &mut self,
        insight_type: MultiDimInsightType,
        result: &crate::ops::operation::OperationResult,
    ) {
        // Find and replace the Computing placeholder
        for insight in &mut self.insights {
            if let Insight::Computing { insight_type: it, .. } = insight {
                if *it == insight_type {
                    // Convert result to insight
                    *insight = match insight_type {
                        MultiDimInsightType::QualityDuplicates => {
                            Insight::QualityDuplicates {
                                dupe_groups: result.succeeded + result.skipped,
                                total_tracks: 0, // Not tracked in result
                                auto_resolvable: result.succeeded,
                            }
                        }
                    };
                    break;
                }
            }
        }

        // Re-sort by priority
        self.insights.sort_by(|a, b| b.priority().cmp(&a.priority()));
    }

    /// Remove Computing placeholder (on error/cancel)
    fn remove_computing(&mut self, insight_type: MultiDimInsightType) {
        self.insights.retain(|i| {
            if let Insight::Computing { insight_type: it, .. } = i {
                *it != insight_type
            } else {
                true
            }
        });
    }

    /// Check if there's a pending background computation
    pub fn has_pending_computation(&self) -> bool {
        self.pending_computation.is_some()
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
    fn test_insights_navigation() {
        let mut state = InsightsViewState::new();
        state.insights = vec![
            Insight::CorpusHealth {
                total_tracks: 100,
                indexed_healthy: true,
                libraries_healthy: true,
                tags_synced: true,
            },
            Insight::DeploymentConflicts { count: 5 },
        ];
        state.list_state = SelectableListState::new()
            .with_selected(Some(0))
            .focused();
        state.list_state.total_items = 2;

        // Navigate down
        state.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
        assert_eq!(state.list_state.selected(), Some(1));

        // Navigate up
        state.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
        assert_eq!(state.list_state.selected(), Some(0));
    }
}
