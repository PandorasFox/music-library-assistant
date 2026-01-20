//! Insights View Module
//!
//! A full-screen view displaying computed insights over health signals.
//! Part of the lateral view ring - can cycle to adjacent views with Tab/Shift-Tab.
//!
//! ## Navigation
//!
//! - Up/Down: Navigate insight list
//! - Enter: Launch flow for selected insight
//! - Tab/Shift-Tab: Cycle to adjacent view
//! - Esc: Return to main menu
//!
//! ## Modal State
//!
//! The view tracks whether the Task Daemon is busy. When busy, actionable
//! insights are dimmed and the Enter key is blocked.

mod render;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::daemon::DaemonStatus;

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
    /// Daemon has operations in-flight - actions blocked
    NotReady_DaemonBusy,
}

impl Default for InsightsModal {
    fn default() -> Self {
        Self::Ready
    }
}

/// State for the insights view
pub struct InsightsViewState {
    /// Modal state tracking daemon busy status
    pub modal: InsightsModal,
}

impl Default for InsightsViewState {
    fn default() -> Self {
        Self {
            modal: InsightsModal::Ready,
        }
    }
}

impl InsightsViewState {
    /// Create a new insights view state
    pub fn new() -> Self {
        Self::default()
    }

    /// Update state every tick - checks daemon status
    pub fn update(&mut self, daemon_status: Option<&DaemonStatus>) {
        let busy = daemon_status
            .map(|s| s.pending > 0)
            .unwrap_or(false);

        self.modal = if busy {
            InsightsModal::NotReady_DaemonBusy
        } else {
            InsightsModal::Ready
        };
    }

    /// Check if daemon is busy (actions should be blocked)
    pub fn is_daemon_busy(&self) -> bool {
        matches!(self.modal, InsightsModal::NotReady_DaemonBusy)
    }

    /// Handle key input
    pub fn handle_key(&mut self, key: KeyEvent) -> InsightsAction {
        match key.code {
            KeyCode::Esc => InsightsAction::RequestQuit,

            KeyCode::Up | KeyCode::Char('k') => {
                // TODO: Navigate list when implemented
                InsightsAction::None
            }

            KeyCode::Down | KeyCode::Char('j') => {
                // TODO: Navigate list when implemented
                InsightsAction::None
            }

            KeyCode::Home => {
                // TODO: Navigate to start when implemented
                InsightsAction::None
            }

            KeyCode::End => {
                // TODO: Navigate to end when implemented
                InsightsAction::None
            }

            KeyCode::Enter => {
                // Block launch if daemon is busy
                if self.is_daemon_busy() {
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
    fn test_daemon_busy_blocks_enter() {
        let mut state = InsightsViewState::new();

        // Not busy - Enter should launch flow
        state.modal = InsightsModal::Ready;
        let action = state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::LaunchFlow);

        // Busy - Enter should be blocked
        state.modal = InsightsModal::NotReady_DaemonBusy;
        let action = state.handle_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::None);
    }

    #[test]
    fn test_update_daemon_status() {
        let mut state = InsightsViewState::new();

        // No status - should be Ready
        state.update(None);
        assert_eq!(state.modal, InsightsModal::Ready);

        // Pending > 0 - should be busy
        let busy_status = DaemonStatus {
            pending: 5,
            ..Default::default()
        };
        state.update(Some(&busy_status));
        assert_eq!(state.modal, InsightsModal::NotReady_DaemonBusy);

        // Pending = 0 - should be ready again
        let idle_status = DaemonStatus {
            pending: 0,
            ..Default::default()
        };
        state.update(Some(&idle_status));
        assert_eq!(state.modal, InsightsModal::Ready);
    }

    #[test]
    fn test_tab_navigation_not_blocked() {
        let mut state = InsightsViewState::new();
        state.modal = InsightsModal::NotReady_DaemonBusy;

        // Tab should still work even when daemon is busy
        let action = state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(action, InsightsAction::CycleNext);

        let action = state.handle_key(KeyEvent::new(KeyCode::Tab, KeyModifiers::SHIFT));
        assert_eq!(action, InsightsAction::CyclePrev);
    }
}
