//! History lateral view — browse tag edit sessions and reverse edits.
//!
//! Two-level StandardList navigation:
//! - **Level 1 (SessionList)**: browse sessions, Z for summary popup, Enter to drill in
//! - **Level 2 (SessionDetail)**: individual edits with multi-select, Z for diff pane,
//!   Enter to initiate reversal
//!
//! Reversal generates standard `TagOp`s routed through the witnessed-decision pipeline.

pub mod render;

use std::collections::{BTreeSet, HashMap};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;

use crate::meta::views::{EditHistoryData, EditRecord, EditSessionSummary};
use crate::ui::input::InputAction;
use crate::ui::widgets::ListClickTargets;
use crate::ui::widgets::rich_text::{RichBlock, RichSpan};
use crate::ui::widgets::standard_list::{
    ListEntry, ListInputResult, StandardListConfig, StandardListState,
};
use crate::ui::widgets::wizard::{WizardItem, WizardOffer};

// ============================================================================
// Actions
// ============================================================================

/// Actions produced by history view key dispatch.
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
// List Entry Types
// ============================================================================

/// Level 1 list entry: an edit session.
pub(crate) struct SessionListEntry {
    pub summary: EditSessionSummary,
}

/// Action from confirming a session list entry.
pub(crate) enum SessionAction {
    Expand(String),
}

impl WizardItem for SessionListEntry {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let s = &self.summary;
        let relative = relative_timestamp(&s.earliest_at);
        Some(WizardOffer::Popup(vec![Line::styled(
            relative,
            Style::default().fg(Color::Cyan),
        )]))
    }
}

/// Parse a `YYYY-MM-DD HH:MM:SS` UTC timestamp into a human-relative string.
fn relative_timestamp(ts: &str) -> String {
    use chrono::{NaiveDateTime, Utc};

    let Ok(naive) = NaiveDateTime::parse_from_str(ts.trim(), "%Y-%m-%d %H:%M:%S") else {
        return ts.to_string();
    };
    let then = naive.and_utc();
    let now = Utc::now();
    let delta = now.signed_duration_since(then);

    if delta.num_seconds() < 0 {
        return "just now".to_string();
    }

    let secs = delta.num_seconds();
    if secs < 60 {
        return "just now".to_string();
    }
    let mins = delta.num_minutes();
    if mins < 60 {
        return format!("{} min{} ago", mins, if mins == 1 { "" } else { "s" });
    }
    let hours = delta.num_hours();
    if hours < 24 {
        return format!("{} hour{} ago", hours, if hours == 1 { "" } else { "s" });
    }
    let days = delta.num_days();
    if days < 30 {
        return format!("{} day{} ago", days, if days == 1 { "" } else { "s" });
    }
    if days < 365 {
        let months = days / 30;
        return format!("{} month{} ago", months, if months == 1 { "" } else { "s" });
    }
    let years = days / 365;
    format!("{} year{} ago", years, if years == 1 { "" } else { "s" })
}

impl ListEntry for SessionListEntry {
    type Action = SessionAction;

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<SessionAction> {
        Some(SessionAction::Expand(self.summary.session_id.clone()))
    }
}

/// Level 2 list entry: an individual edit within a session.
pub(crate) struct EditDetailEntry {
    pub edit: EditRecord,
    pub path: String,
}

/// Action from confirming in the edit detail list.
pub(crate) enum EditDetailAction {
    InitiateReversal,
}

impl WizardItem for EditDetailEntry {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let e = &self.edit;
        let old_val = e.old_value.as_deref().unwrap_or("∅");
        let new_val = e.new_value.as_deref().unwrap_or("∅");

        let headers = vec![
            RichSpan::new(
                "",
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            ),
            RichSpan::new(
                "",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
        ];

        let rows = vec![
            vec![
                vec![RichSpan::new("Field", Style::default().fg(Color::DarkGray))],
                vec![RichSpan::new(
                    &e.field_name,
                    Style::default().fg(Color::Cyan),
                )],
            ],
            vec![
                vec![RichSpan::new("File", Style::default().fg(Color::DarkGray))],
                vec![RichSpan::new(&self.path, Style::default().fg(Color::White))],
            ],
            vec![
                vec![RichSpan::new("Old", Style::default().fg(Color::DarkGray))],
                vec![RichSpan::new(old_val, Style::default().fg(Color::Red))],
            ],
            vec![
                vec![RichSpan::new("New", Style::default().fg(Color::DarkGray))],
                vec![RichSpan::new(new_val, Style::default().fg(Color::Green))],
            ],
            vec![
                vec![RichSpan::new("Time", Style::default().fg(Color::DarkGray))],
                vec![RichSpan::new(
                    crate::ui::helpers::truncate_right(&e.edited_at, 19),
                    Style::default().fg(Color::White),
                )],
            ],
        ];

        let title = format!("{} — {}", e.field_name, self.path);

        Some(WizardOffer::Pane {
            title,
            content: vec![RichBlock::Table {
                headers,
                rows,
                col_ratio: vec![20, 80],
            }],
        })
    }
}

impl ListEntry for EditDetailEntry {
    type Action = EditDetailAction;

    fn on_confirm(&self, selected: &BTreeSet<usize>) -> Option<EditDetailAction> {
        if selected.is_empty() {
            return None;
        }
        Some(EditDetailAction::InitiateReversal)
    }
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

/// Expanded detail of a single session.
pub(crate) struct EditDetailState {
    /// The session_id being viewed
    pub session_id: String,
    /// List entries for individual edits
    pub entries: Vec<EditDetailEntry>,
    /// StandardList state for detail navigation
    pub detail_list: StandardListState,
}

/// Main view state for the History lateral view.
pub(crate) struct HistoryViewState {
    /// Session list entries
    pub sessions: Vec<SessionListEntry>,
    /// StandardList state for session navigation
    pub session_list: StandardListState,
    /// Expanded session detail (loaded on-demand via one-shot query)
    pub detail: Option<EditDetailState>,
    /// Phase of the view
    pub phase: HistoryPhase,
}

/// State for conflict resolution before reversal.
pub(crate) struct ConflictResolutionState {
    pub clean_reversals: Vec<ReversalItem>,
    pub conflicts: Vec<ConflictItem>,
    pub conflict_cursor: usize,
    pub conflict_scroll: usize,
    pub click_targets: ListClickTargets,
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
            session_list: StandardListState::new(StandardListConfig::default()),
            detail: None,
            phase: HistoryPhase::SessionList,
        }
    }

    /// Handle mouse click (delegates to appropriate StandardList).
    pub fn handle_click(&mut self, x: u16, y: u16) {
        match self.phase {
            HistoryPhase::SessionList => {
                self.session_list.handle_click(x, y, &self.sessions);
            }
            HistoryPhase::SessionDetail => {
                if let Some(ref mut detail) = self.detail {
                    detail.detail_list.handle_click(x, y, &detail.entries);
                }
            }
            HistoryPhase::ConflictResolution(ref mut cr) => {
                // Conflict resolution still uses bespoke click targets
                if let Some(id) = cr.click_targets.hit_test(x, y) {
                    if let Ok(idx) = id.parse::<usize>() {
                        if idx < cr.conflicts.len() {
                            cr.conflict_cursor = idx;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Update cached session data from cache thread.
    pub fn update(&mut self, data: Option<EditHistoryData>) {
        if let Some(d) = data {
            self.sessions = d
                .sessions
                .into_iter()
                .map(|s| SessionListEntry { summary: s })
                .collect();
            self.session_list.clamp_cursor(&self.sessions);
        }
    }

    /// Set detail after one-shot DB query.
    pub fn set_detail(
        &mut self,
        session_id: String,
        edits: Vec<EditRecord>,
        inode_paths: HashMap<i64, String>,
    ) {
        let entries: Vec<EditDetailEntry> = edits
            .into_iter()
            .map(|edit| {
                let path = inode_paths
                    .get(&edit.inode)
                    .cloned()
                    .unwrap_or_else(|| "?".to_string());
                EditDetailEntry { edit, path }
            })
            .collect();

        let mut detail_list =
            StandardListState::new(StandardListConfig { multi_select: true });
        // Select all by default
        for i in 0..entries.len() {
            detail_list.selected.insert(i);
        }

        self.detail = Some(EditDetailState {
            session_id,
            entries,
            detail_list,
        });
        self.phase = HistoryPhase::SessionDetail;
    }

    /// Enter conflict resolution phase.
    pub fn set_conflict_resolution(
        &mut self,
        clean: Vec<ReversalItem>,
        conflicts: Vec<ConflictItem>,
    ) {
        self.phase = HistoryPhase::ConflictResolution(ConflictResolutionState {
            clean_reversals: clean,
            conflicts,
            conflict_cursor: 0,
            conflict_scroll: 0,
            click_targets: Default::default(),
        });
    }
}

// ============================================================================
// Key Handling
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
        match self.session_list.handle_input(action, &self.sessions) {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                HistoryAction::None
            }
            ListInputResult::Confirm(SessionAction::Expand(session_id)) => {
                HistoryAction::ExpandSession(session_id)
            }
            ListInputResult::Unhandled => {
                // Handle actions StandardList doesn't know about
                match action {
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
        }
    }

    fn handle_session_detail_input(&mut self, action: &InputAction) -> HistoryAction {
        let detail = match self.detail {
            Some(ref mut d) => d,
            None => return HistoryAction::CollapseDetail,
        };

        match detail.detail_list.handle_input(action, &detail.entries) {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                HistoryAction::None
            }
            ListInputResult::Confirm(EditDetailAction::InitiateReversal) => {
                HistoryAction::InitiateReversal
            }
            ListInputResult::Unhandled => match action {
                InputAction::CycleNext => HistoryAction::CycleNext,
                InputAction::CyclePrev => HistoryAction::CyclePrev,
                InputAction::Cancel => {
                    self.detail = None;
                    self.phase = HistoryPhase::SessionList;
                    HistoryAction::None
                }
                _ => HistoryAction::None,
            },
        }
    }
}

/// Shared input handler for all jettison confirmation phases.
fn handle_jettison_confirm_input(
    action: &InputAction,
    confirm_action: HistoryAction,
) -> HistoryAction {
    match action {
        InputAction::Confirm => confirm_action,
        InputAction::Cancel => HistoryAction::CancelJettison,
        _ => HistoryAction::None,
    }
}

fn handle_conflict_resolution_input(
    state: &mut ConflictResolutionState,
    action: &InputAction,
) -> HistoryAction {
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
        InputAction::Toggle => HistoryAction::ToggleConflictDisposition(state.conflict_cursor),
        InputAction::Confirm => HistoryAction::ConfirmReversal,
        InputAction::Cancel => HistoryAction::CancelConflictResolution,
        _ => HistoryAction::None,
    }
}
