//! History view action handler.
//!
//! Handles session expansion, edit reversal with conflict detection,
//! and mutation staging through the witnessed-decision pipeline.

use std::io::Write;

use crate::db::types::Zone;
use crate::meta::decisions::DecisionKey;
use crate::meta::mutations::Mutation;
use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
use crate::meta::mutations::TagOp;
use crate::meta::views::EditHistoryExportRow;
use crate::ui::{ActiveView, widgets};
use crate::ui::history_view::{
    ConflictDisposition, ConflictItem, HistoryAction, HistoryPhase,
    JettisonAllState, JettisonSessionState, ReversalItem,
};
use super::witness;
use super::super::App;

impl App {
    /// Handle history view actions.
    pub(super) fn handle_history_action(
        &mut self,
        action: HistoryAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            HistoryAction::None => {}

            HistoryAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::History.next(self.transactions_open()));
            }
            HistoryAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::History.prev(self.transactions_open()));
            }

            HistoryAction::RequestQuit => {
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    self.view = ActiveView::ExitConfirm(super::super::ExitConfirmModalState::default());
                }
            }

            HistoryAction::ExpandSession(session_id) => {
                self.expand_history_session(session_id);
            }

            HistoryAction::CollapseDetail => {
                // Handled inline by key handler (sets phase back to SessionList)
            }

            HistoryAction::InitiateReversal => {
                self.initiate_history_reversal();
            }

            HistoryAction::ToggleConflictDisposition(idx) => {
                if let ActiveView::History(ref mut state) = self.view {
                    if let HistoryPhase::ConflictResolution(ref mut cr) = state.phase {
                        if let Some(conflict) = cr.conflicts.get_mut(idx) {
                            conflict.disposition = match conflict.disposition {
                                ConflictDisposition::RevertAnyway => ConflictDisposition::Skip,
                                ConflictDisposition::Skip => ConflictDisposition::RevertAnyway,
                            };
                        }
                    }
                }
            }

            HistoryAction::ConfirmReversal => {
                if let Some(g) = witness {
                    self.confirm_history_reversal(g);
                }
            }

            HistoryAction::CancelConflictResolution => {
                if let ActiveView::History(ref mut state) = self.view {
                    state.phase = HistoryPhase::SessionDetail;
                }
            }

            HistoryAction::JettisonSession => {
                self.enter_jettison_session();
            }
            HistoryAction::ConfirmJettisonSession => {
                self.execute_jettison_session();
            }
            HistoryAction::JettisonAll => {
                self.enter_jettison_all();
            }
            HistoryAction::AdvanceJettisonAll => {
                if let ActiveView::History(ref mut state) = self.view {
                    if let HistoryPhase::ConfirmJettisonAll(ja) = std::mem::replace(
                        &mut state.phase,
                        HistoryPhase::SessionList,
                    ) {
                        state.phase = HistoryPhase::ConfirmJettisonAllFinal(ja);
                    }
                }
            }
            HistoryAction::ConfirmJettisonAll => {
                self.execute_jettison_all();
            }
            HistoryAction::CancelJettison => {
                if let ActiveView::History(ref mut state) = self.view {
                    state.phase = HistoryPhase::SessionList;
                }
            }
        }
    }

    /// Expand a session: one-shot query for edits + inode paths.
    fn expand_history_session(&mut self, session_id: String) {
        let sid = session_id.clone();
        let result = self.cache.query(move |db| {
            let edits = db.get_session_edits(&sid).unwrap_or_default();
            let inodes: Vec<i64> = edits.iter().map(|e| e.inode).collect();

            // Resolve inode → path for display (batch query)
            let inode_paths = db.get_file_paths_batch(crate::db::types::Zone::Corpus, &inodes)
                .unwrap_or_default();

            (edits, inode_paths)
        }).recv();

        if let ActiveView::History(ref mut state) = self.view {
            state.set_detail(session_id, result.0, result.1);
        }
    }

    /// Initiate reversal: detect conflicts between selected edits and current tag state.
    fn initiate_history_reversal(&mut self) {
        // Collect selected edits from the detail view
        let selected_edits = {
            let state = match self.view {
                ActiveView::History(ref state) => state,
                _ => return,
            };
            let detail = match state.detail {
                Some(ref d) => d,
                None => return,
            };
            let mut edits = Vec::new();
            let mut indices: Vec<usize> = detail.selected.iter().copied().collect();
            indices.sort();
            for idx in indices {
                if let Some(edit) = detail.edits.get(idx) {
                    edits.push(edit.clone());
                }
            }
            edits
        };

        if selected_edits.is_empty() {
            return;
        }

        // Query current tag values for conflict detection
        let edits_for_query = selected_edits.clone();
        let current_values = self.cache.query(move |db| {
            let mut results = Vec::new();
            for edit in &edits_for_query {
                // Look up the current value for this (inode, field_name)
                let tags = db.get_corpus_tags(edit.inode).unwrap_or_default();
                let current = tags.iter()
                    .find(|t| t.tag_name.eq_ignore_ascii_case(&edit.field_name))
                    .map(|t| t.tag_value.clone());
                results.push(current);
            }
            results
        }).recv();

        // Classify edits as clean reversals or conflicts
        let mut clean = Vec::new();
        let mut conflicts = Vec::new();

        for (edit, current) in selected_edits.into_iter().zip(current_values.into_iter()) {
            // A clean reversal: current value matches what the edit set it to
            let is_clean = match (&current, &edit.new_value) {
                (Some(cur), Some(new_val)) => cur == new_val,
                (None, None) => true,
                _ => false,
            };

            if is_clean {
                clean.push(ReversalItem { edit });
            } else {
                conflicts.push(ConflictItem {
                    edit,
                    current_value: current,
                    disposition: ConflictDisposition::Skip,
                });
            }
        }

        if let ActiveView::History(ref mut state) = self.view {
            state.set_conflict_resolution(clean, conflicts);
        }
    }

    /// Confirm reversal: generate TagOps from clean reversals + accepted conflicts.
    fn confirm_history_reversal(&mut self, gesture: &witness::ConfirmationGesture) {
        let ops = {
            let state = match self.view {
                ActiveView::History(ref state) => state,
                _ => return,
            };
            let cr = match state.phase {
                HistoryPhase::ConflictResolution(ref cr) => cr,
                _ => return,
            };

            let mut ops = Vec::new();

            // Clean reversals: swap new→old
            for item in &cr.clean_reversals {
                if let Some(op) = reversal_op(&item.edit) {
                    ops.push(op);
                }
            }

            // Accepted conflicts: revert from current to old
            for item in &cr.conflicts {
                if item.disposition != ConflictDisposition::RevertAnyway {
                    continue;
                }
                if let Some(op) = conflict_reversal_op(&item.edit, &item.current_value) {
                    ops.push(op);
                }
            }

            ops
        };

        if ops.is_empty() {
            // Nothing to revert (all conflicts skipped and no clean reversals)
            if let ActiveView::History(ref mut state) = self.view {
                state.phase = HistoryPhase::SessionDetail;
            }
            self.status_message = Some("No reversals to apply".to_string());
            return;
        }

        let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops,
            zone: Zone::Corpus,
        });

        let session_label = {
            let state = match self.view {
                ActiveView::History(ref state) => state,
                _ => return,
            };
            state.detail.as_ref()
                .map(|d| d.session_id.clone())
                .unwrap_or_else(|| "unknown".to_string())
        };

        let key = DecisionKey::EditReversal { session_label: session_label.clone() };
        let label = format!("Reverse edits from session {}", session_label);

        self.stage_mutations_with_transaction(vec![mutation], &label, key, gesture);
        self.after_staging_decisions();
    }

    // =========================================================================
    // Jettison (export + delete edit history)
    // =========================================================================

    /// Enter confirm-jettison-session phase for the currently selected session.
    fn enter_jettison_session(&mut self) {
        let (session_id, edit_count) = match self.view {
            ActiveView::History(ref state) => {
                match state.sessions.get(state.cursor) {
                    Some(s) => (s.session_id.clone(), s.edit_count),
                    None => return,
                }
            }
            _ => return,
        };

        if let ActiveView::History(ref mut state) = self.view {
            state.phase = HistoryPhase::ConfirmJettisonSession(JettisonSessionState {
                session_id,
                edit_count,
            });
        }
    }

    /// Enter first jettison-all confirmation phase.
    fn enter_jettison_all(&mut self) {
        let (total_records, session_count) = match self.view {
            ActiveView::History(ref state) => {
                let total: usize = state.sessions.iter().map(|s| s.edit_count).sum();
                (total, state.sessions.len())
            }
            _ => return,
        };

        if let ActiveView::History(ref mut state) = self.view {
            state.phase = HistoryPhase::ConfirmJettisonAll(JettisonAllState {
                total_records,
                session_count,
            });
        }
    }

    /// Execute jettison for a single session: export to log, delete from DB.
    fn execute_jettison_session(&mut self) {
        let session_id = match self.view {
            ActiveView::History(ref state) => {
                match state.phase {
                    HistoryPhase::ConfirmJettisonSession(ref js) => js.session_id.clone(),
                    _ => return,
                }
            }
            _ => return,
        };

        // Query the session's edit records for export
        let sid = session_id.clone();
        let rows = self.cache.query(move |db| {
            db.get_session_edit_history(&sid).unwrap_or_default()
        }).recv();

        if rows.is_empty() {
            self.status_message = Some("No records to export".to_string());
            if let ActiveView::History(ref mut state) = self.view {
                state.phase = HistoryPhase::SessionList;
            }
            return;
        }

        // Export to log file
        match export_to_log(&rows) {
            Ok(path) => {
                // Send delete to write thread
                if let Some(sender) = crate::db::write_thread::signal_sender() {
                    sender.clear_tag_edit_history_session(&session_id);
                }

                let count = rows.len();
                self.status_message = Some(format!(
                    "Jettisoned {} record{} → {}",
                    count,
                    if count == 1 { "" } else { "s" },
                    path,
                ));

                // Reset view state
                if let ActiveView::History(ref mut state) = self.view {
                    state.sessions.retain(|s| s.session_id != session_id);
                    if !state.sessions.is_empty() && state.cursor >= state.sessions.len() {
                        state.cursor = state.sessions.len() - 1;
                    }
                    state.phase = HistoryPhase::SessionList;
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Export failed: {}", e));
                if let ActiveView::History(ref mut state) = self.view {
                    state.phase = HistoryPhase::SessionList;
                }
            }
        }
    }

    /// Execute jettison-all: export everything to log, delete all from DB.
    fn execute_jettison_all(&mut self) {
        // Query all edit history for export
        let rows = self.cache.query(move |db| {
            db.get_all_edit_history().unwrap_or_default()
        }).recv();

        if rows.is_empty() {
            self.status_message = Some("No records to export".to_string());
            if let ActiveView::History(ref mut state) = self.view {
                state.phase = HistoryPhase::SessionList;
            }
            return;
        }

        // Export to log file
        match export_to_log(&rows) {
            Ok(path) => {
                // Send delete-all to write thread
                if let Some(sender) = crate::db::write_thread::signal_sender() {
                    sender.clear_tag_edit_history();
                }

                let count = rows.len();
                self.status_message = Some(format!(
                    "Jettisoned all {} record{} → {}",
                    count,
                    if count == 1 { "" } else { "s" },
                    path,
                ));

                // Reset view state
                if let ActiveView::History(ref mut state) = self.view {
                    state.sessions.clear();
                    state.cursor = 0;
                    state.detail = None;
                    state.phase = HistoryPhase::SessionList;
                }
            }
            Err(e) => {
                self.status_message = Some(format!("Export failed: {}", e));
                if let ActiveView::History(ref mut state) = self.view {
                    state.phase = HistoryPhase::SessionList;
                }
            }
        }
    }
}

/// Export edit history rows to a tab-separated log file.
///
/// Returns the path to the written file on success.
fn export_to_log(rows: &[EditHistoryExportRow]) -> Result<String, String> {
    let logs_dir = mm_utils::paths::get_logs_dir()
        .map_err(|e| format!("Cannot resolve logs dir: {}", e))?;

    let now = chrono::Local::now();
    let filename = format!("tag_edit_history_export_{}.log", now.format("%Y-%m-%d_%H%M%S"));
    let path = logs_dir.join(&filename);

    let mut file = std::fs::File::create(&path)
        .map_err(|e| format!("Cannot create {}: {}", path.display(), e))?;

    // Header
    writeln!(file, "id\tinode\tfield_name\told_value\tnew_value\tedited_at\tsession_id")
        .map_err(|e| format!("Write error: {}", e))?;

    // Data rows
    for row in rows {
        writeln!(
            file,
            "{}\t{}\t{}\t{}\t{}\t{}\t{}",
            row.id,
            row.inode,
            row.field_name,
            row.old_value.as_deref().unwrap_or(""),
            row.new_value.as_deref().unwrap_or(""),
            row.edited_at,
            row.session_id,
        )
        .map_err(|e| format!("Write error: {}", e))?;
    }

    Ok(path.to_string_lossy().into_owned())
}

/// Generate a TagOp for a clean reversal (current == new_value, revert to old_value).
fn reversal_op(edit: &crate::meta::views::EditRecord) -> Option<TagOp> {
    match (&edit.old_value, &edit.new_value) {
        // Was replace: old→new, revert: new→old
        (Some(old), Some(new)) => {
            Some(TagOp::replace_tag(edit.inode, &edit.field_name, new.clone(), old.clone()))
        }
        // Was add (old=None, new=Some): revert by dropping the added value
        (None, Some(new)) => {
            Some(TagOp::drop_tag(edit.inode, &edit.field_name, new.clone()))
        }
        // Was drop (old=Some, new=None): revert by adding the dropped value back
        (Some(old), None) => {
            Some(TagOp::add_tag(edit.inode, &edit.field_name, old.clone()))
        }
        // No-op
        (None, None) => None,
    }
}

/// Generate a TagOp for a conflict reversal (current != new_value, revert to old_value anyway).
fn conflict_reversal_op(
    edit: &crate::meta::views::EditRecord,
    current_value: &Option<String>,
) -> Option<TagOp> {
    match (&edit.old_value, current_value) {
        // Current exists, old existed: replace current→old
        (Some(old), Some(cur)) => {
            Some(TagOp::replace_tag(edit.inode, &edit.field_name, cur.clone(), old.clone()))
        }
        // Old existed, current is gone: add old back
        (Some(old), None) => {
            Some(TagOp::add_tag(edit.inode, &edit.field_name, old.clone()))
        }
        // Old was None (was an add), current exists: drop current
        (None, Some(cur)) => {
            Some(TagOp::drop_tag(edit.inode, &edit.field_name, cur.clone()))
        }
        // Both None: no-op
        (None, None) => None,
    }
}
