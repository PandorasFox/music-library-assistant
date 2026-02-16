//! Inbox view action handlers.
//!
//! Handles Enter (stash matched / edit tags) and T (edit tags) actions
//! from the inbox lateral view.

use crate::ui::active_view::ActiveView;
use super::witness;
use super::App;

impl App {
    pub(super) fn handle_inbox_action(&mut self, action: super::super::inbox_view::InboxAction, witness: Option<&witness::DecisionWitness>) {
        use crate::corpus::paths;
        use crate::meta::mutations::Mutation;
        use crate::meta::mutations::file_ops::MoveToStashMutation;
        use crate::meta::mutations::indexing::DropFromIndexMutation;
        use super::super::inbox_view::{InboxAction, InboxEntryStatus};

        match action {
            InboxAction::None => {}
            InboxAction::RequestQuit => {
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    self.view = ActiveView::ExitConfirm(super::super::ExitConfirmModalState::default());
                }
            }
            InboxAction::CycleNext => {
                self.start_lateral_view(crate::ui::widgets::LateralView::Inbox.next());
            }
            InboxAction::CyclePrev => {
                self.start_lateral_view(crate::ui::widgets::LateralView::Inbox.prev());
            }
            InboxAction::LaunchSelected => {
                // Extract entry info from current view (borrow ends here)
                let entry_info = match &self.view {
                    ActiveView::Inbox(ref state) => {
                        state.selected_entry().map(|e| {
                            let is_match = matches!(e.status, InboxEntryStatus::CorpusMatch(_));
                            (e.inode, e.path.clone(), is_match)
                        })
                    }
                    _ => None,
                };

                if let Some((inode, path, is_match)) = entry_info {
                    if is_match {
                        // Matched file: stage MoveToStash + DropFromIndex → transaction review
                        let Some(w) = witness else { return };
                        let resolver = paths::get_resolver();
                        let abs_path = resolver.resolve(std::path::Path::new(&path));
                        let mutations = vec![
                            Mutation::MoveToStash(MoveToStashMutation {
                                path: abs_path,
                                stash_name: "inbox_matches".to_string(),
                            }),
                            Mutation::DropFromIndex(DropFromIndexMutation {
                                path: std::path::PathBuf::from(&path),
                                inode: Some(inode),
                                zone: Some("inbox".to_string()),
                            }),
                        ];
                        self.stage_mutations_with_transaction(mutations, "Stash inbox match", w);
                        self.start_transaction_review();
                    } else {
                        // Healthy/Unindexed file: open tag editor
                        self.launch_inbox_tag_editor(&path);
                    }
                }
            }
            InboxAction::EditTags => {
                // T key: open tag editor for any selected file
                let path = match &self.view {
                    ActiveView::Inbox(ref state) => {
                        state.selected_entry().map(|e| e.path.clone())
                    }
                    _ => None,
                };
                if let Some(path) = path {
                    self.launch_inbox_tag_editor(&path);
                }
            }
        }
    }

    /// Open the tag editor for an inbox file, pushing the current view onto the stack.
    fn launch_inbox_tag_editor(&mut self, rel_path: &str) {
        let resolver = crate::corpus::paths::get_resolver();
        let abs_path = resolver.resolve(std::path::Path::new(rel_path));
        self.push_current_view();
        self.start_tag_editor_for_path(&abs_path, false);
    }
}
