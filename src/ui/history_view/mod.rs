//! History lateral view — browse tag edit sessions and reverse edits.
//!
//! Two-phase navigation:
//! - **SessionList**: browse sessions from `tag_edit_history`, grouped by session_id
//! - **SessionDetail**: expand a session to see individual edits, select for reversal
//!
//! Reversal generates standard `TagOp`s routed through the witnessed-decision pipeline.

pub mod render;

use std::collections::{HashMap, HashSet};

use crate::ui::input::InputAction;

use crate::meta::views::{EditHistoryData, EditRecord, EditSessionSummary};

// ============================================================================
// Actions
// ============================================================================

/// Actions produced by Phase 1 key dispatch.
pub(crate) enum HistoryAction {
    None,
    /// Tab → next lateral view
    CycleNext,
    /// Shift-Tab → previous lateral view
    CyclePrev,
    /// Esc from session list → quit request
    RequestQuit,
    /// Enter on session → expand to detail
    ExpandSession(String),
    /// Esc from detail → back to session list
    CollapseDetail,
    /// Enter in detail → initiate reversal of selected edits
    InitiateReversal,
    /// In conflict resolution: toggle disposition for a conflict
    ToggleConflictDisposition(usize),
    /// In conflict resolution: confirm reversal with resolved conflicts
    ConfirmReversal,
    /// Esc from conflict resolution → back to detail
    CancelConflictResolution,
    /// `d` in session list → enter confirm-jettison-session phase
    JettisonSession,
    /// Enter in ConfirmJettisonSession → execute jettison
    ConfirmJettisonSession,
    /// `D` in session list → enter first jettison-all confirm
    JettisonAll,
    /// Enter in ConfirmJettisonAll → advance to second confirm
    AdvanceJettisonAll,
    /// Enter in ConfirmJettisonAllFinal → execute jettison all
    ConfirmJettisonAll,
    /// Esc from any jettison confirm → back to session list
    CancelJettison,
}

// ============================================================================
// State Types
// ============================================================================

/// Phase of the history view.
pub(crate) enum HistoryPhase {
    /// Browsing sessions list
    SessionList,
    /// Viewing detail of a single session (expanded)
    SessionDetail,
    /// Conflict resolution before reversal
    ConflictResolution(ConflictResolutionState),
    /// Single confirm for jettisoning the selected session
    ConfirmJettisonSession(JettisonSessionState),
    /// First "are you sure?" for jettisoning all history
    ConfirmJettisonAll(JettisonAllState),
    /// Second "are you REALLY sure?" for jettisoning all history
    ConfirmJettisonAllFinal(JettisonAllState),
}

/// State for per-session jettison confirmation.
pub(crate) struct JettisonSessionState {
    pub session_id: String,
    pub edit_count: usize,
}

/// State for jettison-all confirmation.
pub(crate) struct JettisonAllState {
    pub total_records: usize,
    pub session_count: usize,
}

/// Main view state for the History lateral view.
pub(crate) struct HistoryViewState {
    /// Session list (from cache)
    pub sessions: Vec<EditSessionSummary>,
    /// Selected session index
    pub cursor: usize,
    pub scroll: usize,
    /// Expanded session detail (loaded on-demand via one-shot query)
    pub detail: Option<SessionDetail>,
    /// Phase of the view
    pub phase: HistoryPhase,
    /// Click targets for the currently visible list (set during render).
    pub click_targets: crate::ui::widgets::ListClickTargets,
}

/// Expanded detail of a single session.
pub(crate) struct SessionDetail {
    /// The session_id being viewed
    pub session_id: String,
    /// All edits in the session
    pub edits: Vec<EditRecord>,
    /// Resolved paths for inodes (for display)
    pub inode_paths: HashMap<i64, String>,
    /// Which individual edits are selected for reversal (indices into edits)
    pub selected: HashSet<usize>,
    /// Cursor within the detail list
    pub detail_cursor: usize,
    pub detail_scroll: usize,
}

/// State for conflict resolution before reversal.
pub(crate) struct ConflictResolutionState {
    pub clean_reversals: Vec<ReversalItem>,
    pub conflicts: Vec<ConflictItem>,
    pub conflict_cursor: usize,
    pub conflict_scroll: usize,
}

/// An edit that can be cleanly reversed (current value matches what the edit set).
pub(crate) struct ReversalItem {
    pub edit: EditRecord,
}

/// An edit where the current value differs from what the edit set (tag changed since).
pub(crate) struct ConflictItem {
    pub edit: EditRecord,
    pub current_value: Option<String>,
    /// User decision: revert to old_value anyway, or skip
    pub disposition: ConflictDisposition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConflictDisposition {
    /// Revert from current_value to edit.old_value
    RevertAnyway,
    /// Skip this edit
    Skip,
}

// ============================================================================
// Construction & Update
// ============================================================================

impl HistoryViewState {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            cursor: 0,
            scroll: 0,
            detail: None,
            phase: HistoryPhase::SessionList,
            click_targets: Default::default(),
        }
    }

    /// Handle mouse click for cursor selection.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                match &mut self.phase {
                    HistoryPhase::SessionList => {
                        if idx < self.sessions.len() {
                            self.cursor = idx;
                        }
                    }
                    HistoryPhase::SessionDetail => {
                        if let Some(ref mut detail) = self.detail {
                            if idx < detail.edits.len() {
                                detail.detail_cursor = idx;
                            }
                        }
                    }
                    HistoryPhase::ConflictResolution(ref mut cr) => {
                        if idx < cr.conflicts.len() {
                            cr.conflict_cursor = idx;
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    /// Update cached session data from cache thread.
    pub fn update(&mut self, data: Option<EditHistoryData>) {
        if let Some(d) = data {
            self.sessions = d.sessions;
            // Clamp cursor
            if !self.sessions.is_empty() && self.cursor >= self.sessions.len() {
                self.cursor = self.sessions.len() - 1;
            }
        }
    }

    /// Set detail after one-shot DB query.
    pub fn set_detail(&mut self, session_id: String, edits: Vec<EditRecord>, inode_paths: HashMap<i64, String>) {
        let edit_count = edits.len();
        let all_selected: HashSet<usize> = (0..edit_count).collect();
        self.detail = Some(SessionDetail {
            session_id,
            edits,
            inode_paths,
            selected: all_selected,
            detail_cursor: 0,
            detail_scroll: 0,
        });
        self.phase = HistoryPhase::SessionDetail;
    }

    /// Enter conflict resolution phase.
    pub fn set_conflict_resolution(&mut self, clean: Vec<ReversalItem>, conflicts: Vec<ConflictItem>) {
        self.phase = HistoryPhase::ConflictResolution(ConflictResolutionState {
            clean_reversals: clean,
            conflicts,
            conflict_cursor: 0,
            conflict_scroll: 0,
        });
    }
}

// ============================================================================
// Key Handling (Phase 1)
// ============================================================================

impl HistoryViewState {
    pub fn handle_input(&mut self, action: &InputAction) -> HistoryAction {
        match self.phase {
            HistoryPhase::SessionList => self.handle_session_list_input(action),
            HistoryPhase::SessionDetail => self.handle_session_detail_input(action),
            HistoryPhase::ConflictResolution(ref mut state) => {
                handle_conflict_resolution_input(state, action)
            }
            HistoryPhase::ConfirmJettisonSession(_) => {
                handle_jettison_confirm_input(action, HistoryAction::ConfirmJettisonSession)
            }
            HistoryPhase::ConfirmJettisonAll(_) => {
                handle_jettison_confirm_input(action, HistoryAction::AdvanceJettisonAll)
            }
            HistoryPhase::ConfirmJettisonAllFinal(_) => {
                handle_jettison_confirm_input(action, HistoryAction::ConfirmJettisonAll)
            }
        }
    }

    fn handle_session_list_input(&mut self, action: &InputAction) -> HistoryAction {
        match action {
            InputAction::NavUp => {
                if self.cursor > 0 {
                    self.cursor -= 1;
                }
                HistoryAction::None
            }
            InputAction::NavDown => {
                if !self.sessions.is_empty() && self.cursor < self.sessions.len() - 1 {
                    self.cursor += 1;
                }
                HistoryAction::None
            }
            InputAction::Home => {
                self.cursor = 0;
                HistoryAction::None
            }
            InputAction::End => {
                if !self.sessions.is_empty() {
                    self.cursor = self.sessions.len() - 1;
                }
                HistoryAction::None
            }
            InputAction::Confirm => {
                if let Some(session) = self.sessions.get(self.cursor) {
                    HistoryAction::ExpandSession(session.session_id.clone())
                } else {
                    HistoryAction::None
                }
            }
            InputAction::Char('d') => {
                if self.sessions.is_empty() {
                    HistoryAction::None
                } else {
                    HistoryAction::JettisonSession
                }
            }
            InputAction::Char('D') => {
                if self.sessions.is_empty() {
                    HistoryAction::None
                } else {
                    HistoryAction::JettisonAll
                }
            }
            InputAction::CycleNext => HistoryAction::CycleNext,
            InputAction::CyclePrev => HistoryAction::CyclePrev,
            InputAction::Cancel => HistoryAction::RequestQuit,
            _ => HistoryAction::None,
        }
    }

    fn handle_session_detail_input(&mut self, action: &InputAction) -> HistoryAction {
        let detail = match self.detail {
            Some(ref mut d) => d,
            None => return HistoryAction::CollapseDetail,
        };

        match action {
            InputAction::NavUp => {
                if detail.detail_cursor > 0 {
                    detail.detail_cursor -= 1;
                }
                HistoryAction::None
            }
            InputAction::NavDown => {
                if !detail.edits.is_empty() && detail.detail_cursor < detail.edits.len() - 1 {
                    detail.detail_cursor += 1;
                }
                HistoryAction::None
            }
            InputAction::Home => {
                detail.detail_cursor = 0;
                HistoryAction::None
            }
            InputAction::End => {
                if !detail.edits.is_empty() {
                    detail.detail_cursor = detail.edits.len() - 1;
                }
                HistoryAction::None
            }
            InputAction::Toggle => {
                let idx = detail.detail_cursor;
                if idx < detail.edits.len() {
                    if detail.selected.contains(&idx) {
                        detail.selected.remove(&idx);
                    } else {
                        detail.selected.insert(idx);
                    }
                }
                HistoryAction::None
            }
            InputAction::Confirm => {
                if detail.selected.is_empty() {
                    HistoryAction::None
                } else {
                    HistoryAction::InitiateReversal
                }
            }
            InputAction::CycleNext => HistoryAction::CycleNext,
            InputAction::CyclePrev => HistoryAction::CyclePrev,
            InputAction::Cancel => {
                self.detail = None;
                self.phase = HistoryPhase::SessionList;
                HistoryAction::None
            }
            _ => HistoryAction::None,
        }
    }
}

/// Shared input handler for all jettison confirmation phases.
fn handle_jettison_confirm_input(action: &InputAction, confirm_action: HistoryAction) -> HistoryAction {
    match action {
        InputAction::Confirm => confirm_action,
        InputAction::Cancel => HistoryAction::CancelJettison,
        _ => HistoryAction::None,
    }
}

fn handle_conflict_resolution_input(state: &mut ConflictResolutionState, action: &InputAction) -> HistoryAction {
    match action {
        InputAction::NavUp => {
            if state.conflict_cursor > 0 {
                state.conflict_cursor -= 1;
            }
            HistoryAction::None
        }
        InputAction::NavDown => {
            if !state.conflicts.is_empty() && state.conflict_cursor < state.conflicts.len() - 1 {
                state.conflict_cursor += 1;
            }
            HistoryAction::None
        }
        InputAction::Toggle => {
            HistoryAction::ToggleConflictDisposition(state.conflict_cursor)
        }
        InputAction::Confirm => HistoryAction::ConfirmReversal,
        InputAction::Cancel => HistoryAction::CancelConflictResolution,
        _ => HistoryAction::None,
    }
}
