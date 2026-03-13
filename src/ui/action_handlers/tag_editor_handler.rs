//! Unified tag editor action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::ui::active_view::ActiveView;
use crate::ui::tag_editor;

impl HandleAction for tag_editor::UnifiedTagEditorAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        use tag_editor::UnifiedTagEditorAction;

        match self {
            UnifiedTagEditorAction::None => {}
            UnifiedTagEditorAction::CloseModal => {}

            UnifiedTagEditorAction::StageDecisionAndNavigate {
                key,
                mutations,
                direction,
            } => {
                use crate::ui::tag_editor::types::NavigationDirection;
                let is_embedded =
                    matches!(&app.view, ActiveView::UnifiedTagEditor(ref e) if e.is_embedded());

                if is_embedded {
                    // Embedded mode: track locally, don't stage to transaction
                    if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                        editor.set_staged_mutations(mutations);
                        editor.staged_decision_count += 1;
                    }
                } else {
                    // Standalone mode: stage to transaction (requires witness)
                    let Some(w) = witness else { return };
                    app.stage_tag_editor_decision(key, mutations, w);
                }

                // Navigate in both modes
                if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                    match direction {
                        NavigationDirection::Next => {
                            if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                                editor.current_item_idx += 1;
                                editor.reset_field_state();
                            }
                        }
                        NavigationDirection::Prev => {
                            if editor.current_item_idx > 0 {
                                editor.current_item_idx -= 1;
                                editor.reset_field_state();
                            }
                        }
                    }
                }
            }

            UnifiedTagEditorAction::StageDecisionAndReview { key, mutations } => {
                let Some(w) = witness else { return };
                // Stage the decision AND immediately show transaction review
                // Used for aggregated mode or single-item contexts
                app.stage_tag_editor_decision(key, mutations, w);

                // Transition to standardized review modal
                // Note: tag editor state is preserved on the view stack for Cancel return
                app.after_staging_decisions();
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                // In closed-txn mode, discard the transaction.
                // In open-txn mode, leave the persistent transaction intact.
                if !app.open_txn_mode() {
                    let _ = super::super::operator_decisions::discard_transaction(&mut app.witch);
                }
                // Return to the view that launched the tag editor (e.g., corpus browser)
                if !app.pop_and_restore() {
                    app.start_health_view();
                }
                app.status_message = Some("Edits discarded".to_string());
            }

            UnifiedTagEditorAction::NextItem => {
                if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                    if editor.current_item_idx < editor.total_items.saturating_sub(1) {
                        editor.current_item_idx += 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::PrevItem => {
                if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                    if editor.current_item_idx > 0 {
                        editor.current_item_idx -= 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::StatusMessage(msg) => {
                app.status_message = Some(msg);
            }

            UnifiedTagEditorAction::RequestFillFromDb { inode } => match inode {
                Some(inode) => {
                    let tag_pairs = app
                        .witch
                        .query(crate::db::domain::GetCorpusTags { inode });

                    if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                        editor.fill_from_db_result(tag_pairs);
                    }
                    app.status_message = Some("Tags loaded from database".to_string());
                }
                None => {
                    app.status_message =
                        Some("Track not indexed - no database tags available".to_string());
                }
            },

            UnifiedTagEditorAction::CloseEmbedded => {
                // Return to parent health modal without staging
                if !app.pop_and_restore() {
                    app.start_health_view();
                }
            }

            UnifiedTagEditorAction::StageAndCloseEmbedded {
                decision_key,
                decision_label,
                mutations,
            } => {
                let Some(g) = witness else { return };
                // Stage collected mutations at parent's decision key
                let decision = g.decide(&decision_label, mutations);
                let _ = super::super::operator_decisions::stage_decision(
                    &mut app.witch,
                    decision_key,
                    decision,
                );
                // Return to parent health modal
                if !app.pop_and_restore() {
                    app.start_health_view();
                }
            }

            UnifiedTagEditorAction::RequestTransactionReview => {
                // Transition to standardized review modal
                // Note: tag editor state is preserved on the view stack for Cancel return
                app.after_staging_decisions();
            }
        }
    }
}
