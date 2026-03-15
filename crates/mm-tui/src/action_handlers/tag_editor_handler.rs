//! Unified tag editor action handler.

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::active_view::ActiveView;
use crate::tag_editor;

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
                use crate::tag_editor::types::NavigationDirection;
                let is_embedded =
                    matches!(&app.view, ActiveView::UnifiedTagEditor(ref e) if e.is_embedded());

                if is_embedded {
                    if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                        editor.set_staged_mutations(mutations);
                        editor.core.staged_decision_count += 1;
                    }
                } else {
                    let Some(w) = witness else { return };
                    app.stage_tag_editor_decision(key, mutations, w);
                }

                if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                    match direction {
                        NavigationDirection::Next => {
                            if editor.core.current_file
                                < editor.audio_files.len().saturating_sub(1)
                            {
                                editor.core.current_file += 1;
                                editor.reset_field_state();
                            }
                        }
                        NavigationDirection::Prev => {
                            if editor.core.current_file > 0 {
                                editor.core.current_file -= 1;
                                editor.reset_field_state();
                            }
                        }
                    }
                }
            }

            UnifiedTagEditorAction::StageDecisionAndReview { key, mutations } => {
                let Some(w) = witness else { return };
                app.stage_tag_editor_decision(key, mutations, w);
                app.after_staging_decisions();
            }

            UnifiedTagEditorAction::DiscardTransaction => {
                if !app.open_txn_mode() {
                    let _ = super::super::operator_decisions::discard_transaction(app);
                }
                if !app.pop_and_restore() {
                    app.start_health_view();
                }
                app.status_message = Some("Edits discarded".to_string());
            }

            UnifiedTagEditorAction::NextItem => {
                if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                    if editor.core.current_file < editor.audio_files.len().saturating_sub(1) {
                        editor.core.current_file += 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::PrevItem => {
                if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                    if editor.core.current_file > 0 {
                        editor.core.current_file -= 1;
                        editor.reset_field_state();
                    }
                }
            }

            UnifiedTagEditorAction::StatusMessage(msg) => {
                app.status_message = Some(msg);
            }

            UnifiedTagEditorAction::RequestFillFromDb { inode } => match inode {
                Some(inode) => {
                    let tag_pairs = app.query(mm_meta::domain_queries::GetCorpusTags { inode });

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

            UnifiedTagEditorAction::RequestFillFromDisk => {
                let query_params = if let ActiveView::UnifiedTagEditor(ref editor) = app.view {
                    editor.current_file_for_disk_query()
                } else {
                    None
                };
                if let Some((inode, zone)) = query_params {
                    let result = app.query(mm_meta::domain_queries::GetFileTagValues {
                        inodes: vec![inode],
                        zone,
                    });
                    let tags = result.into_iter().next().map(|(_, t)| t).unwrap_or_default();
                    if let ActiveView::UnifiedTagEditor(ref mut editor) = app.view {
                        editor.fill_from_disk_with_tags(tags);
                    }
                }
                app.status_message = Some("Tags refreshed from disk".to_string());
            }

            UnifiedTagEditorAction::CloseEmbedded => {
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
                let decision = g.decide(&decision_label, mutations);
                let _ = super::super::operator_decisions::stage_decision(
                    app,
                    decision_key,
                    decision,
                );
                if !app.pop_and_restore() {
                    app.start_health_view();
                }
            }

            UnifiedTagEditorAction::RequestTransactionReview => {
                app.after_staging_decisions();
            }
        }
    }
}
