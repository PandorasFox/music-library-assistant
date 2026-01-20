//! Wait State Helper
//!
//! Standardized pattern for UI components that need to wait for daemon work to complete.
//! Used by progress screens and modals that trigger mutations.
//!
//! ## Usage Pattern
//!
//! ```rust,ignore
//! struct MyModal {
//!     wait_state: WaitState,
//!     // ... other state
//! }
//!
//! impl MyModal {
//!     fn trigger_work(&mut self, daemon: &mut TaskDaemon) {
//!         daemon.queue_computation(...);
//!         self.wait_state.start();
//!     }
//!
//!     fn tick(&mut self, daemon: &TaskDaemon) -> bool {
//!         if self.wait_state.tick(daemon) {
//!             // Work is complete
//!             return true;
//!         }
//!         false
//!     }
//! }
//! ```

use crate::daemon::{DaemonStateSnapshot, TaskDaemon};

/// Helper for waiting on daemon work completion.
///
/// Tracks whether we've seen the daemon working and detects when work completes.
/// This avoids false positives from checking daemon state before work starts.
#[derive(Debug, Clone, Default)]
pub struct WaitState {
    /// Whether we're currently waiting for work to complete.
    waiting: bool,
    /// Whether we've seen daemon enter Working state (to distinguish idle-before from idle-after).
    seen_working: bool,
}

impl WaitState {
    /// Create a new wait state (not waiting).
    pub fn new() -> Self {
        Self {
            waiting: false,
            seen_working: false,
        }
    }

    /// Start waiting for daemon work to complete.
    ///
    /// Call this after queuing work to the daemon.
    pub fn start(&mut self) {
        self.waiting = true;
        self.seen_working = false;
    }

    /// Reset to not-waiting state.
    pub fn reset(&mut self) {
        self.waiting = false;
        self.seen_working = false;
    }

    /// Check if we're currently waiting.
    pub fn is_waiting(&self) -> bool {
        self.waiting
    }

    /// Check if we've seen the daemon working.
    pub fn has_seen_working(&self) -> bool {
        self.seen_working
    }

    /// Tick the wait state, checking for completion.
    ///
    /// Returns `true` when waiting is complete (daemon was working and is now idle/completed).
    /// Call this each frame while waiting.
    pub fn tick(&mut self, daemon: &TaskDaemon) -> bool {
        if !self.waiting {
            return false;
        }

        let status = daemon.status();

        // Track when daemon starts working
        if status.state == DaemonStateSnapshot::Working {
            self.seen_working = true;
        }

        // Check for completion:
        // - Must have seen working state (to avoid false positive from initial idle)
        // - No pending tasks
        // - Daemon is now Idle or Completed
        if self.seen_working && status.pending == 0 {
            match status.state {
                DaemonStateSnapshot::Idle | DaemonStateSnapshot::Completed => {
                    self.waiting = false;
                    return true; // Complete!
                }
                DaemonStateSnapshot::Working => {
                    // Still working, not complete
                }
            }
        }

        false
    }

    /// Tick with additional check for pending DB writes.
    ///
    /// Like `tick()`, but also waits for the DB write queue to drain.
    /// Use this when you need to ensure all side effects are persisted.
    pub fn tick_with_db_drain(&mut self, daemon: &TaskDaemon) -> bool {
        if !self.waiting {
            return false;
        }

        let status = daemon.status();
        let db_queue_empty = daemon.db_queue_depth() == 0;

        // Track when daemon starts working
        if status.state == DaemonStateSnapshot::Working {
            self.seen_working = true;
        }

        // Check for completion:
        // - Must have seen working state
        // - No pending tasks
        // - DB queue is empty
        // - Daemon is now Idle or Completed
        if self.seen_working && status.pending == 0 && db_queue_empty {
            match status.state {
                DaemonStateSnapshot::Idle | DaemonStateSnapshot::Completed => {
                    self.waiting = false;
                    return true; // Complete!
                }
                DaemonStateSnapshot::Working => {
                    // Still working, not complete
                }
            }
        }

        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wait_state_initial() {
        let state = WaitState::new();
        assert!(!state.is_waiting());
        assert!(!state.has_seen_working());
    }

    #[test]
    fn test_wait_state_start() {
        let mut state = WaitState::new();
        state.start();
        assert!(state.is_waiting());
        assert!(!state.has_seen_working());
    }

    #[test]
    fn test_wait_state_reset() {
        let mut state = WaitState::new();
        state.start();
        state.seen_working = true;
        state.reset();
        assert!(!state.is_waiting());
        assert!(!state.has_seen_working());
    }
}
