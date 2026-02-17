//! Deployment Preview Modal
//!
//! Handles the deployment preview: loading deploy data, staging deploy mutations
//! (leftovers, stale, new, conflicts), and the preview action handler.

use crate::corpus::paths;
use crate::meta::decisions::{DecisionKey, DecisionSource};
use crate::meta::mutations::file_ops::{HardLinkMutation, LibraryMoveMutation, MoveToStashMutation};
use crate::ui::{deploy_modal, ActiveView};
use super::witness;
use super::super::App;

impl App {
    /// Handle Deploy lateral view actions.
    pub(super) fn handle_deploy_action(&mut self, action: deploy_modal::DeployAction, witness: Option<&witness::DecisionWitness>) {
        use crate::ui::widgets;

        match action {
            deploy_modal::DeployAction::None => {}
            deploy_modal::DeployAction::CycleNext => {
                self.start_lateral_view(widgets::LateralView::Deploy.next(self.transactions_open()));
            }
            deploy_modal::DeployAction::CyclePrev => {
                self.start_lateral_view(widgets::LateralView::Deploy.prev(self.transactions_open()));
            }
            deploy_modal::DeployAction::Confirm => {
                let Some(w) = witness else { return };
                // Extract cached data from Preview state
                let cached_data = match &self.view {
                    ActiveView::Deploy(deploy_modal::DeployViewState::Preview(ref preview)) => {
                        Some(preview.cached_data.clone())
                    }
                    _ => None,
                };
                if let Some(data) = cached_data {
                    let mutation_count = self.stage_deploy_mutations(&data, w);
                    if mutation_count > 0 {
                        self.after_staging_decisions();
                    } else {
                        self.status_message = Some("No deploy operations needed".to_string());
                    }
                }
            }
            deploy_modal::DeployAction::RequestQuit => {
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    self.view = ActiveView::ExitConfirm(super::super::ExitConfirmModalState::default());
                }
            }
        }
    }

    /// Stage deploy mutations for transaction review.
    ///
    /// Returns the number of mutations staged.
    ///
    /// Paths in the signal data are relative to their roots:
    /// - corpus_path: relative to corpus_root
    /// - deploy_path/library_path/expected_path: relative to libraries_root
    ///
    /// These must be resolved to absolute for filesystem mutations.
    fn stage_deploy_mutations(&mut self, data: &deploy_modal::DeployModalData, _witness: &witness::DecisionWitness) -> usize {
        use crate::meta::mutations::Mutation;

        let open_txn = self.open_txn_mode();
        let Some(ref mut witch) = self.witch else {
            return 0;
        };
        let resolver = paths::get_resolver();
        let mut mutations = Vec::new();

        // ORDERING IS CRITICAL:
        // 1. Leftovers FIRST - stash orphan files to clear destination paths
        // 2. Stale files - move existing deployments to correct paths
        // 3. New files - deploy new hard links (destinations now clear)
        // 4. Conflicts - deploy conflict resolutions

        // 1. Leftover files: move to stash (preserve data, never destroy)
        // library_path is stored as "{library_name}/path/..." in files table
        // Needs "libraries/" prefix for filesystem resolution
        for file in &data.leftover {
            let path_rel = std::path::Path::new("libraries").join(&file.library_path);
            let path = resolver.resolve(&path_rel);
            mutations.push(Mutation::MoveToStash(MoveToStashMutation {
                path,
                stash_name: "library_leftovers".to_string(),
            }));
        }

        // 2. Stale files: move from wrong path to correct path
        // Both library_path and expected_path are stored as "{library_name}/path/..."
        // Just need to prepend "libraries/" for filesystem resolution
        for file in &data.stale {
            let source_rel = std::path::Path::new("libraries").join(&file.library_path);
            let source = resolver.resolve(&source_rel);

            let dest_rel = std::path::Path::new("libraries").join(&file.expected_path);
            let destination = resolver.resolve(&dest_rel);

            mutations.push(Mutation::LibraryMove(LibraryMoveMutation { source, destination }));
        }

        // 3. New files: create hard links
        // library_name is already assigned during DeployModalData::load() via config lookup
        // deploy_path is "Artist/Album/..." (relative to library root)
        for file in &data.new {
            if file.deploy_path.is_empty() || file.library_name.is_empty() {
                continue;
            }

            let source = resolver.resolve(std::path::Path::new(&file.corpus_path));
            let dest_rel = std::path::Path::new("libraries")
                .join(&file.library_name)
                .join(&file.deploy_path);
            let destination = resolver.resolve(&dest_rel);
            mutations.push(Mutation::HardLink(HardLinkMutation { source, destination }));
        }

        // 4. Conflicts: pick first alphabetical corpus path and deploy it
        // Find matching new file's library_name for the deploy_path
        for group in &data.conflicts {
            if let Some((corpus_path, _inode)) = group
                .conflicting_files
                .iter()
                .min_by(|a, b| a.0.cmp(&b.0))
            {
                let source = resolver.resolve(std::path::Path::new(corpus_path));

                // Find library_name from a new file with matching corpus_path
                let library_name = data.new.iter()
                    .find(|f| f.corpus_path == *corpus_path)
                    .map(|f| f.library_name.as_str())
                    .unwrap_or("");

                if library_name.is_empty() {
                    continue;
                }

                let dest_rel = std::path::Path::new("libraries")
                    .join(library_name)
                    .join(&group.deploy_path);
                let destination = resolver.resolve(&dest_rel);
                mutations.push(Mutation::HardLink(HardLinkMutation { source, destination }));
            }
        }

        let count = mutations.len();
        if count == 0 {
            return 0;
        }

        // Start transaction and stage the decision
        if !open_txn {
            let _ = witch.start_transaction("Deploy");
        }
        let _ = super::super::operator_decisions::stage_decision(
            witch,
            DecisionKey::single(DecisionSource::Deploy),
            "Deploy operations",
            mutations,
        );

        count
    }
}
