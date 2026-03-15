//! History view action handler.
//!
//! Handles session expansion, edit reversal with conflict detection,
//! and mutation staging through the witnessed-decision pipeline.

use super::super::App;
use super::witness;
use super::HandleAction;
use mm_meta::db_types::Zone;
use mm_ui::decision_keys;
use mm_meta::mutations::jettison::ExportEditHistoryMutation;
use mm_meta::mutations::tag_edit::ApplyTagOpsMutation;
use mm_meta::mutations::Mutation;
use mm_meta::mutations::TagOp;
use crate::history_view::{
    ConflictDisposition, ConflictItem, HistoryAction, HistoryPhase, JettisonAllState,
    JettisonSessionState, ReversalItem,
};
use crate::ActiveView;

impl HandleAction for HistoryAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            HistoryAction::ExpandSession(session_id) => {
                app.expand_history_session(session_id);
            }

            HistoryAction::CollapseDetail => {
                // Handled inline by key handler (sets phase back to SessionList)
            }

            HistoryAction::InitiateReversal => {
                app.initiate_history_reversal();
            }

            HistoryAction::ToggleConflictDisposition(idx) => {
                if let ActiveView::History(ref mut s) = app.view {
                    if let HistoryPhase::ConflictResolution(ref mut cr) = s.data.phase {
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
                    app.confirm_history_reversal(g);
                }
            }

            HistoryAction::CancelConflictResolution => {
                if let ActiveView::History(ref mut s) = app.view {
                    s.data.phase = HistoryPhase::SessionDetail;
                }
            }

            HistoryAction::JettisonSession => {
                app.enter_jettison_session();
            }
            HistoryAction::ConfirmJettisonSession => {
                if let Some(g) = witness {
                    app.execute_jettison_session(g);
                }
            }
            HistoryAction::JettisonAll => {
                app.enter_jettison_all();
            }
            HistoryAction::AdvanceJettisonAll => {
                if let ActiveView::History(ref mut s) = app.view {
                    if let HistoryPhase::ConfirmJettisonAll(ja) =
                        std::mem::replace(&mut s.data.phase, HistoryPhase::SessionList)
                    {
                        s.data.phase = HistoryPhase::ConfirmJettisonAllFinal(ja);
                    }
                }
            }
            HistoryAction::ConfirmJettisonAll => {
                if let Some(g) = witness {
                    app.execute_jettison_all(g);
                }
            }
            HistoryAction::CancelJettison => {
                if let ActiveView::History(ref mut s) = app.view {
                    s.data.phase = HistoryPhase::SessionList;
                }
            }
        }
    }
}

impl App {
    /// Expand a session: one-shot query for edits + inode paths.
    fn expand_history_session(&mut self, session_id: String) {
        let result = self
            .query(mm_meta::domain_queries::GetSessionEditDetail {
                session_id: session_id.clone(),
            });

        if let ActiveView::History(ref mut s) = self.view {
            s.data.set_detail(session_id, result.edits, result.inode_paths);
        }
    }

    /// Initiate reversal: detect conflicts between selected edits and current tag state.
    fn initiate_history_reversal(&mut self) {
        // Collect selected edits from the detail view
        let selected_edits = {
            let data = match self.view {
                ActiveView::History(ref s) => &s.data,
                _ => return,
            };
            let detail = match data.detail {
                Some(ref d) => d,
                None => return,
            };
            let mut edits = Vec::new();
            let mut indices: Vec<usize> = detail.detail_list.selected.iter().copied().collect();
            indices.sort();
            for idx in indices {
                if let Some(entry) = detail.entries.get(idx) {
                    edits.extend(entry.edits());
                }
            }
            edits
        };

        if selected_edits.is_empty() {
            return;
        }

        // Query current tag values for conflict detection
        let queries: Vec<(i64, String)> = selected_edits
            .iter()
            .map(|e| (e.inode, e.field_name.clone()))
            .collect();
        let current_values = self
            .query(mm_meta::domain_queries::GetCurrentTagValues { queries });

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

        if let ActiveView::History(ref mut s) = self.view {
            s.data.set_conflict_resolution(clean, conflicts);
        }
    }

    /// Confirm reversal: generate TagOps from clean reversals + accepted conflicts.
    fn confirm_history_reversal(&mut self, gesture: &witness::ConfirmationGesture) {
        let ops = {
            let data = match self.view {
                ActiveView::History(ref s) => &s.data,
                _ => return,
            };
            let cr = match data.phase {
                HistoryPhase::ConflictResolution(ref cr) => cr,
                _ => return,
            };

            let mut ops = Vec::new();

            // Clean reversals: swap new->old
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
            if let ActiveView::History(ref mut s) = self.view {
                s.data.phase = HistoryPhase::SessionDetail;
            }
            self.status_message = Some("No reversals to apply".to_string());
            return;
        }

        let mutation = Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops,
            zone: Zone::Corpus,
        });

        let session_label = {
            let data = match self.view {
                ActiveView::History(ref s) => &s.data,
                _ => return,
            };
            data
                .detail
                .as_ref()
                .map(|d| d.session_id.clone())
                .unwrap_or_else(|| "unknown".to_string())
        };

        let key = decision_keys::edit_reversal(session_label.clone());
        let label = format!("Reverse edits from session {}", session_label);

        self.stage_mutations_with_transaction(vec![mutation], &label, key, gesture);
        self.after_staging_decisions();
    }

    // =========================================================================
    // Jettison (export + delete edit history via transaction)
    // =========================================================================

    /// Enter confirm-jettison-session phase for the currently selected session.
    fn enter_jettison_session(&mut self) {
        let (session_id, edit_count) = match self.view {
            ActiveView::History(ref s) => {
                let cursor = s.interaction.session_list.cursor;
                match s.data.sessions.get(cursor) {
                    Some(entry) => (entry.summary.session_id.clone(), entry.summary.edit_count),
                    None => return,
                }
            }
            _ => return,
        };

        if let ActiveView::History(ref mut s) = self.view {
            s.data.phase = HistoryPhase::ConfirmJettisonSession(JettisonSessionState {
                session_id,
                edit_count,
            });
        }
    }

    /// Enter first jettison-all confirmation phase.
    fn enter_jettison_all(&mut self) {
        let (total_records, session_count) = match self.view {
            ActiveView::History(ref s) => {
                let total: usize = s.data.sessions.iter().map(|e| e.summary.edit_count).sum();
                (total, s.data.sessions.len())
            }
            _ => return,
        };

        if let ActiveView::History(ref mut s) = self.view {
            s.data.phase = HistoryPhase::ConfirmJettisonAll(JettisonAllState {
                total_records,
                session_count,
            });
        }
    }

    /// Stage jettison for a single session via the transaction system.
    fn execute_jettison_session(&mut self, gesture: &witness::ConfirmationGesture) {
        let session_id = match self.view {
            ActiveView::History(ref s) => match s.data.phase {
                HistoryPhase::ConfirmJettisonSession(ref js) => js.session_id.clone(),
                _ => return,
            },
            _ => return,
        };

        let mutation = Mutation::ExportEditHistory(ExportEditHistoryMutation {
            session_id: Some(session_id.clone()),
        });
        let label = format!("Jettison edit history: session {}", session_id);

        self.stage_mutations_with_transaction(
            vec![mutation],
            &label,
            decision_keys::jettison_edit_history(),
            gesture,
        );

        // Return to session list and navigate to transaction review
        if let ActiveView::History(ref mut s) = self.view {
            s.data.phase = HistoryPhase::SessionList;
        }
        self.after_staging_decisions();
    }

    /// Stage jettison-all via the transaction system.
    fn execute_jettison_all(&mut self, gesture: &witness::ConfirmationGesture) {
        let mutation = Mutation::ExportEditHistory(ExportEditHistoryMutation {
            session_id: None,
        });
        let label = "Jettison edit history: all sessions".to_string();

        self.stage_mutations_with_transaction(
            vec![mutation],
            &label,
            decision_keys::jettison_edit_history(),
            gesture,
        );

        // Return to session list and navigate to transaction review
        if let ActiveView::History(ref mut s) = self.view {
            s.data.phase = HistoryPhase::SessionList;
        }
        self.after_staging_decisions();
    }
}

/// Generate a TagOp for a clean reversal (current == new_value, revert to old_value).
fn reversal_op(edit: &mm_meta::views::EditRecord) -> Option<TagOp> {
    match (&edit.old_value, &edit.new_value) {
        // Was replace: old->new, revert: new->old
        (Some(old), Some(new)) => Some(TagOp::replace_tag(
            edit.inode,
            &edit.field_name,
            new.clone(),
            old.clone(),
        )),
        // Was add (old=None, new=Some): revert by dropping the added value
        (None, Some(new)) => Some(TagOp::drop_tag(edit.inode, &edit.field_name, new.clone())),
        // Was drop (old=Some, new=None): revert by adding the dropped value back
        (Some(old), None) => Some(TagOp::add_tag(edit.inode, &edit.field_name, old.clone())),
        // No-op
        (None, None) => None,
    }
}

/// Generate a TagOp for a conflict reversal (current != new_value, revert to old_value anyway).
fn conflict_reversal_op(
    edit: &mm_meta::views::EditRecord,
    current_value: &Option<String>,
) -> Option<TagOp> {
    match (&edit.old_value, current_value) {
        // Current exists, old existed: replace current->old
        (Some(old), Some(cur)) => Some(TagOp::replace_tag(
            edit.inode,
            &edit.field_name,
            cur.clone(),
            old.clone(),
        )),
        // Old existed, current is gone: add old back
        (Some(old), None) => Some(TagOp::add_tag(edit.inode, &edit.field_name, old.clone())),
        // Old was None (was an add), current exists: drop current
        (None, Some(cur)) => Some(TagOp::drop_tag(edit.inode, &edit.field_name, cur.clone())),
        // Both None: no-op
        (None, None) => None,
    }
}
