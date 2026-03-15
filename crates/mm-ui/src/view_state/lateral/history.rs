//! History view state: data + interaction bundled.
//!
//! Two-level StandardList navigation:
//! - **Level 1 (SessionList)**: browse sessions, Z for summary popup, Enter to drill in
//! - **Level 2 (SessionDetail)**: individual edits with multi-select, Z for diff pane,
//!   Enter to initiate reversal
//!
//! Reversal generates standard `TagOp`s routed through the witnessed-decision pipeline.

use std::collections::{BTreeSet, HashMap};

use ratatui::style::{Color, Style};
use ratatui::text::Line;

use mm_meta::views::{EditHistoryData, EditRecord, EditSessionSummary};

use crate::click_targets::ListClickTargets;
use crate::helpers::truncate_right;
use crate::input::InputAction;
use crate::rich_text::{RichBlock, RichSpan};
use crate::route::HistoryRoute;
use crate::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::view_state::ViewCore;
use crate::wizard::{WizardItem, WizardOffer};

// ============================================================================
// Actions
// ============================================================================

/// Domain actions produced by history view key dispatch.
///
/// Protocol actions (CycleNext, CyclePrev, Cancel-as-quit) are handled centrally.
pub enum HistoryAction {
    /// Enter on session -> expand to detail
    ExpandSession(String),
    /// Esc from detail -> back to session list
    CollapseDetail,
    /// Enter in detail -> initiate reversal of selected edits
    InitiateReversal,
    /// In conflict resolution: toggle disposition for a conflict
    ToggleConflictDisposition(usize),
    /// In conflict resolution: confirm reversal with resolved conflicts
    ConfirmReversal,
    /// Esc from conflict resolution -> back to detail
    CancelConflictResolution,
    /// `d` in session list -> enter confirm-jettison-session phase
    JettisonSession,
    /// Enter in ConfirmJettisonSession -> execute jettison
    ConfirmJettisonSession,
    /// `D` in session list -> enter first jettison-all confirm
    JettisonAll,
    /// Enter in ConfirmJettisonAll -> advance to second confirm
    AdvanceJettisonAll,
    /// Enter in ConfirmJettisonAllFinal -> execute jettison all
    ConfirmJettisonAll,
    /// Esc from any jettison confirm -> back to session list
    CancelJettison,
}

// ============================================================================
// List Entry Types
// ============================================================================

/// Level 1 list entry: an edit session.
pub struct SessionListEntry {
    pub summary: EditSessionSummary,
}

/// Action from confirming a session list entry.
pub enum SessionAction {
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
pub fn relative_timestamp(ts: &str) -> String {
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

/// Level 2 list entry: grouped view of edits within a session.
///
/// Edits that are identical across multiple files (same field+old+new) are
/// pulled out as `CommonEdit` entries. Remaining per-file edits are grouped
/// under `FileHeader` separators with individual `FileEdit` rows.
pub enum EditDetailEntry {
    /// An edit pattern shared across multiple files.
    CommonEdit {
        field_name: String,
        old_value: Option<String>,
        new_value: Option<String>,
        /// Affected file paths (for wizard pane).
        paths: Vec<String>,
        /// The underlying EditRecords (for reversal).
        edits: Vec<EditRecord>,
    },
    /// Visual separator before per-file edits.
    PerFileSeparator,
    /// Non-selectable file path header grouping per-file edits.
    FileHeader {
        path: String,
    },
    /// An individual edit that wasn't aggregable.
    FileEdit {
        edit: EditRecord,
        path: String,
    },
}

impl EditDetailEntry {
    /// Return the EditRecords this entry represents (for reversal).
    pub fn edits(&self) -> Vec<EditRecord> {
        match self {
            EditDetailEntry::CommonEdit { edits, .. } => edits.clone(),
            EditDetailEntry::FileEdit { edit, .. } => vec![edit.clone()],
            EditDetailEntry::PerFileSeparator | EditDetailEntry::FileHeader { .. } => vec![],
        }
    }
}

/// Action from confirming in the edit detail list.
pub enum EditDetailAction {
    InitiateReversal,
}

impl WizardItem for EditDetailEntry {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        match self {
            EditDetailEntry::CommonEdit {
                field_name,
                old_value,
                new_value,
                paths,
                ..
            } => {
                let old = old_value.as_deref().unwrap_or("\u{2205}");
                let new = new_value.as_deref().unwrap_or("\u{2205}");

                let mut content = vec![
                    RichBlock::Paragraph(vec![
                        RichSpan::new(field_name, Style::default().fg(Color::Cyan)),
                        RichSpan::new(": ", Style::default().fg(Color::DarkGray)),
                        RichSpan::new(old, Style::default().fg(Color::Red)),
                        RichSpan::new(" \u{2192} ", Style::default().fg(Color::DarkGray)),
                        RichSpan::new(new, Style::default().fg(Color::Green)),
                    ]),
                    RichBlock::Blank,
                    RichBlock::Heading(format!("Affected files ({})", paths.len())),
                    RichBlock::Separator,
                ];

                for path in paths {
                    content.push(RichBlock::Paragraph(vec![RichSpan::new(
                        path,
                        Style::default().fg(Color::White),
                    )]));
                }

                let title = format!(
                    "{}: {} \u{2192} {} ({} files)",
                    field_name,
                    old,
                    new,
                    paths.len()
                );

                Some(WizardOffer::Pane { title, content })
            }
            EditDetailEntry::FileEdit { edit, path } => {
                let old_val = edit.old_value.as_deref().unwrap_or("\u{2205}");
                let new_val = edit.new_value.as_deref().unwrap_or("\u{2205}");

                let kv_table = |label: &str, value: &str, style: Style| {
                    vec![
                        vec![RichSpan::new(label, Style::default().fg(Color::DarkGray))],
                        vec![RichSpan::new(value, style)],
                    ]
                };

                let headers = vec![
                    RichSpan::new("", Style::default()),
                    RichSpan::new("", Style::default()),
                ];

                let rows = vec![
                    kv_table("Field", &edit.field_name, Style::default().fg(Color::Cyan)),
                    kv_table("File", path, Style::default().fg(Color::White)),
                    kv_table("Old", old_val, Style::default().fg(Color::Red)),
                    kv_table("New", new_val, Style::default().fg(Color::Green)),
                    kv_table(
                        "Time",
                        &truncate_right(&edit.edited_at, 19),
                        Style::default().fg(Color::White),
                    ),
                ];

                let title = format!("{} \u{2014} {}", edit.field_name, path);

                Some(WizardOffer::Pane {
                    title,
                    content: vec![RichBlock::Table {
                        headers,
                        rows,
                        col_ratio: vec![12, 88],
                    }],
                })
            }
            EditDetailEntry::PerFileSeparator | EditDetailEntry::FileHeader { .. } => None,
        }
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

    fn is_selectable(&self) -> bool {
        matches!(
            self,
            EditDetailEntry::CommonEdit { .. } | EditDetailEntry::FileEdit { .. }
        )
    }
}

// ============================================================================
// State Types
// ============================================================================

/// Phase of the history view.
pub enum HistoryPhase {
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
pub struct JettisonSessionState {
    pub session_id: String,
    pub edit_count: usize,
}

/// State for jettison-all confirmation.
pub struct JettisonAllState {
    pub total_records: usize,
    pub session_count: usize,
}

/// Expanded detail of a single session.
pub struct EditDetailState {
    /// The session_id being viewed
    pub session_id: String,
    /// List entries for individual edits
    pub entries: Vec<EditDetailEntry>,
    /// StandardList state for detail navigation
    pub detail_list: StandardListState,
}

/// State for conflict resolution before reversal.
pub struct ConflictResolutionState {
    pub clean_reversals: Vec<ReversalItem>,
    pub conflicts: Vec<ConflictItem>,
    pub conflict_cursor: usize,
    pub conflict_scroll: usize,
    pub click_targets: ListClickTargets,
}

/// An edit that can be cleanly reversed (current value matches what the edit set).
pub struct ReversalItem {
    pub edit: EditRecord,
}

/// An edit where the current value differs from what the edit set (tag changed since).
pub struct ConflictItem {
    pub edit: EditRecord,
    pub current_value: Option<String>,
    /// User decision: revert to old_value anyway, or skip
    pub disposition: ConflictDisposition,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ConflictDisposition {
    /// Revert from current_value to edit.old_value
    RevertAnyway,
    /// Skip this edit
    Skip,
}

// ============================================================================
// HistoryViewData — server-fetched data + multi-phase state machine
// ============================================================================

/// Data for the History lateral view.
///
/// Owns the multi-phase state machine (session list, detail, conflict resolution,
/// jettison confirmation). Interaction state (session_list cursor/scroll) lives
/// in `HistoryInteraction`.
pub struct HistoryViewData {
    /// Session list entries
    pub sessions: Vec<SessionListEntry>,
    /// Expanded session detail (loaded on-demand via one-shot query)
    pub detail: Option<EditDetailState>,
    /// Phase of the view
    pub phase: HistoryPhase,
}

// ============================================================================
// Construction & Update
// ============================================================================

impl HistoryViewData {
    pub fn new() -> Self {
        Self {
            sessions: Vec::new(),
            detail: None,
            phase: HistoryPhase::SessionList,
        }
    }

    /// Handle mouse click (delegates to appropriate StandardList).
    pub fn handle_click(&mut self, session_list: &mut StandardListState, x: u16, y: u16) {
        match self.phase {
            HistoryPhase::SessionList => {
                session_list.handle_click(x, y, &self.sessions);
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
    pub fn update(&mut self, session_list: &mut StandardListState, data: Option<EditHistoryData>) {
        if let Some(d) = data {
            self.sessions = d
                .sessions
                .into_iter()
                .map(|s| SessionListEntry { summary: s })
                .collect();
            session_list.clamp_cursor(&self.sessions);
        }
    }

    /// Set detail after one-shot DB query.
    ///
    /// Groups edits: identical `(field, old, new)` tuples across 2+ inodes
    /// become `CommonEdit` entries; remaining per-file edits are grouped
    /// under `FileHeader` separators.
    pub fn set_detail(
        &mut self,
        session_id: String,
        edits: Vec<EditRecord>,
        inode_paths: HashMap<i64, String>,
    ) {
        let entries = group_edits(edits, &inode_paths);

        let mut detail_list = StandardListState::new(StandardListConfig {
            multi_select: true,
            pane_min_width: 70,
        });
        // Select all selectable entries by default
        for (i, entry) in entries.iter().enumerate() {
            if entry.is_selectable() {
                detail_list.selected.insert(i);
            }
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
// Grouping Logic
// ============================================================================

/// Key for grouping edits by identical mutation pattern.
#[derive(Hash, PartialEq, Eq, Clone)]
struct EditPatternKey {
    field_name: String,
    old_value: Option<String>,
    new_value: Option<String>,
}

/// Group edits into common (multi-file) and per-file entries.
pub fn group_edits(
    edits: Vec<EditRecord>,
    inode_paths: &HashMap<i64, String>,
) -> Vec<EditDetailEntry> {
    use std::collections::hash_map::Entry;

    // Phase 1: bucket edits by (field, old, new) pattern.
    let mut buckets: HashMap<EditPatternKey, Vec<EditRecord>> = HashMap::new();
    let mut insertion_order: Vec<EditPatternKey> = Vec::new();

    for edit in edits {
        let key = EditPatternKey {
            field_name: edit.field_name.clone(),
            old_value: edit.old_value.clone(),
            new_value: edit.new_value.clone(),
        };
        match buckets.entry(key.clone()) {
            Entry::Vacant(e) => {
                insertion_order.push(key);
                e.insert(vec![edit]);
            }
            Entry::Occupied(mut e) => {
                e.get_mut().push(edit);
            }
        }
    }

    // Phase 2: partition into common (2+ distinct inodes) vs per-file.
    let mut common_entries = Vec::new();
    let mut per_file_edits: Vec<(EditRecord, String)> = Vec::new();

    for key in insertion_order {
        let bucket = buckets.remove(&key).unwrap();
        let distinct_inodes: std::collections::HashSet<i64> =
            bucket.iter().map(|e| e.inode).collect();

        if distinct_inodes.len() >= 2 {
            let mut paths: Vec<String> = distinct_inodes
                .iter()
                .map(|inode| {
                    inode_paths
                        .get(inode)
                        .cloned()
                        .unwrap_or_else(|| "?".to_string())
                })
                .collect();
            paths.sort();

            common_entries.push(EditDetailEntry::CommonEdit {
                field_name: key.field_name,
                old_value: key.old_value,
                new_value: key.new_value,
                paths,
                edits: bucket,
            });
        } else {
            for edit in bucket {
                let path = inode_paths
                    .get(&edit.inode)
                    .cloned()
                    .unwrap_or_else(|| "?".to_string());
                per_file_edits.push((edit, path));
            }
        }
    }

    // Phase 3: group per-file edits by inode.
    let mut entries = common_entries;

    if !per_file_edits.is_empty() {
        if !entries.is_empty() {
            entries.push(EditDetailEntry::PerFileSeparator);
        }

        // Group by inode, preserving order of first appearance.
        let mut inode_groups: Vec<(i64, String, Vec<EditRecord>)> = Vec::new();
        let mut inode_index: HashMap<i64, usize> = HashMap::new();

        for (edit, path) in per_file_edits {
            match inode_index.entry(edit.inode) {
                Entry::Vacant(e) => {
                    let idx = inode_groups.len();
                    e.insert(idx);
                    inode_groups.push((edit.inode, path, vec![edit]));
                }
                Entry::Occupied(e) => {
                    inode_groups[*e.get()].2.push(edit);
                }
            }
        }

        for (_inode, path, edits) in inode_groups {
            if edits.len() > 1 {
                // Multiple edits for one file — show header + individual edits
                entries.push(EditDetailEntry::FileHeader {
                    path: path.clone(),
                });
                for edit in edits {
                    entries.push(EditDetailEntry::FileEdit { edit, path: path.clone() });
                }
            } else {
                // Single edit for a file — just show the edit directly
                let edit = edits.into_iter().next().unwrap();
                entries.push(EditDetailEntry::FileEdit { edit, path });
            }
        }
    }

    entries
}

// ============================================================================
// Key Handling
// ============================================================================

impl HistoryViewData {
    pub fn handle_input(
        &mut self,
        session_list: &mut StandardListState,
        action: &InputAction,
    ) -> Option<HistoryAction> {
        match self.phase {
            HistoryPhase::SessionList => self.handle_session_list_input(session_list, action),
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

    fn handle_session_list_input(
        &mut self,
        session_list: &mut StandardListState,
        action: &InputAction,
    ) -> Option<HistoryAction> {
        match session_list.handle_input(action, &self.sessions) {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                None
            }
            ListInputResult::Confirm(SessionAction::Expand(session_id)) => {
                Some(HistoryAction::ExpandSession(session_id))
            }
            ListInputResult::Unhandled => {
                // Handle actions StandardList doesn't know about
                match action {
                    InputAction::Char('d') => {
                        if self.sessions.is_empty() {
                            None
                        } else {
                            Some(HistoryAction::JettisonSession)
                        }
                    }
                    InputAction::Char('D') => {
                        if self.sessions.is_empty() {
                            None
                        } else {
                            Some(HistoryAction::JettisonAll)
                        }
                    }
                    _ => None,
                }
            }
        }
    }

    fn handle_session_detail_input(&mut self, action: &InputAction) -> Option<HistoryAction> {
        let detail = match self.detail {
            Some(ref mut d) => d,
            None => return Some(HistoryAction::CollapseDetail),
        };

        match detail.detail_list.handle_input(action, &detail.entries) {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                None
            }
            ListInputResult::Confirm(EditDetailAction::InitiateReversal) => {
                Some(HistoryAction::InitiateReversal)
            }
            ListInputResult::Unhandled => match action {
                InputAction::Cancel => {
                    self.detail = None;
                    self.phase = HistoryPhase::SessionList;
                    None
                }
                _ => None,
            },
        }
    }
}

/// Shared input handler for all jettison confirmation phases.
fn handle_jettison_confirm_input(
    action: &InputAction,
    confirm_action: HistoryAction,
) -> Option<HistoryAction> {
    match action {
        InputAction::Confirm => Some(confirm_action),
        InputAction::Cancel => Some(HistoryAction::CancelJettison),
        _ => None,
    }
}

fn handle_conflict_resolution_input(
    state: &mut ConflictResolutionState,
    action: &InputAction,
) -> Option<HistoryAction> {
    match action {
        InputAction::NavUp => {
            if state.conflict_cursor > 0 {
                state.conflict_cursor -= 1;
            }
            None
        }
        InputAction::NavDown => {
            if !state.conflicts.is_empty() && state.conflict_cursor < state.conflicts.len() - 1 {
                state.conflict_cursor += 1;
            }
            None
        }
        InputAction::Toggle => Some(HistoryAction::ToggleConflictDisposition(state.conflict_cursor)),
        InputAction::Confirm => Some(HistoryAction::ConfirmReversal),
        InputAction::Cancel => Some(HistoryAction::CancelConflictResolution),
        _ => None,
    }
}

// ============================================================================
// HistoryInteraction — UI navigation state
// ============================================================================

/// Interaction state for the history view.
pub struct HistoryInteraction {
    pub session_list: StandardListState,
}

impl ViewCore for HistoryInteraction {
    type Route = HistoryRoute;
    type Action = ();
    type Data = ();

    fn from_route(_route: &HistoryRoute) -> Self {
        Self {
            session_list: StandardListState::new(StandardListConfig::default()),
        }
    }

    fn to_route(&self) -> HistoryRoute {
        HistoryRoute { session: None }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<()> {
        None
    }
}

impl HistoryInteraction {
    pub fn new() -> Self {
        Self {
            session_list: StandardListState::new(StandardListConfig::default()),
        }
    }
}

// ============================================================================
// HistoryViewState — bundled data + interaction
// ============================================================================

/// Complete view state for the history view.
///
/// Bundles server-fetched data with interaction state so that ActiveView
/// carries a single struct instead of loose `{ data, interaction }` fields.
pub struct HistoryViewState {
    pub data: HistoryViewData,
    pub interaction: HistoryInteraction,
}

impl HistoryViewState {
    /// Create a new history view state with empty data.
    pub fn new() -> Self {
        Self {
            data: HistoryViewData::new(),
            interaction: HistoryInteraction::new(),
        }
    }

    /// Create and populate from fetched data.
    pub fn with_data(history_data: EditHistoryData) -> Self {
        let mut state = Self::new();
        state.data.update(&mut state.interaction.session_list, Some(history_data));
        state
    }

    /// Handle a semantic input action, delegating to data's multi-phase state machine.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<HistoryAction> {
        self.data.handle_input(&mut self.interaction.session_list, action)
    }

    /// Handle mouse click.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        self.data.handle_click(&mut self.interaction.session_list, x, y);
    }

    /// Update cached session data from cache thread.
    pub fn update(&mut self, data: Option<EditHistoryData>) {
        self.data.update(&mut self.interaction.session_list, data);
    }

    /// Route serialization.
    pub fn to_route(&self) -> HistoryRoute {
        self.interaction.to_route()
    }

    /// Restore position from a route.
    pub fn apply_route(&mut self, route: &HistoryRoute) {
        if let Some(session_id) = route.session {
            let target = session_id.to_string();
            for (i, entry) in self.data.sessions.iter().enumerate() {
                if entry.summary.session_id == target {
                    self.interaction.session_list.cursor = i;
                    break;
                }
            }
        }
    }
}
