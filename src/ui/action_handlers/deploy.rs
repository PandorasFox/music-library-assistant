//! Deployment Preview Modal
//!
//! Handles the deployment preview: loading deploy data, staging deploy mutations
//! (leftovers, stale, new, conflicts), and the preview action handler.

use crate::corpus::paths;
use crate::meta::mutations::file_ops::{HardLinkMutation, LibraryMoveMutation, MoveToStashMutation};
use crate::ui::{deploy_modal, transaction_review, ActiveView};
use super::super::App;

impl App {
    /// Start deployment preview from Insights view.
    pub(in crate::ui) fn start_deployment_preview_from_insights(&mut self) {
        let data = self.witch.as_mut()
            .and_then(|w| {
                let read_db = w.read_db();
                deploy_modal::DeployModalData::load(&read_db).ok()
            })
            .unwrap_or_default();

        let preview = deploy_modal::DeploymentPreviewState::new(data);
        self.view = ActiveView::DeploymentPreview(preview);
    }

    /// Handle deployment preview actions.
    pub(in crate::ui) fn handle_deployment_preview_action(&mut self, action: deploy_modal::DeploymentPreviewAction) {
        match action {
            deploy_modal::DeploymentPreviewAction::None => {}
            deploy_modal::DeploymentPreviewAction::Confirm => {
                // Generate deploy mutations and stage for review
                // Clone the cached data to avoid borrow issues
                let cached_data = match &self.view {
                    ActiveView::DeploymentPreview(ref preview) => Some(preview.cached_data.clone()),
                    _ => None,
                };
                if let Some(data) = cached_data {
                    let mutation_count = self.stage_deploy_mutations(&data);
                    if mutation_count > 0 {
                        // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                        self.start_transaction_review(transaction_review::TransactionReviewSource::DeployPreview);
                    } else {
                        // No mutations (edge case) - go directly to Insights
                        self.start_insights_view();
                        self.status_message = Some("No deploy operations needed".to_string());
                    }
                } else {
                    self.start_insights_view();
                }
            }
            deploy_modal::DeploymentPreviewAction::Cancel => {
                self.cancel_and_return_to_insights("Deployment preview cancelled");
                self.status_message = Some("Deployment cancelled".to_string());
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
    fn stage_deploy_mutations(&mut self, data: &deploy_modal::DeployModalData) -> usize {
        use crate::meta::mutations::Mutation;

        let Some(ref mut witch) = self.witch else {
            return 0;
        };
        let resolver = paths::get_resolver();
        let config = crate::config::load_config().ok();
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
        // corpus_path is "corpus/..." and deploy_path is "Artist/Album/..."
        // We need to determine target library from config and build full path
        for file in &data.new {
            // Skip files with empty deploy_path (data integrity check)
            if file.deploy_path.is_empty() {
                continue;
            }

            // Source: corpus path resolves directly
            let source = resolver.resolve(std::path::Path::new(&file.corpus_path));

            // Destination: look up target library from config, build full path
            let corpus_path = std::path::Path::new(&file.corpus_path);
            let library_name = config
                .as_ref()
                .and_then(|c| c.get_libraries_for_corpus_path(corpus_path).first().cloned());

            let dest_rel = if let Some(lib) = library_name {
                std::path::Path::new("libraries")
                    .join(&lib)
                    .join(&file.deploy_path)
            } else {
                // Fallback: use first configured library or skip
                // This shouldn't happen if signals are correctly generated
                continue;
            };
            let destination = resolver.resolve(&dest_rel);
            mutations.push(Mutation::HardLink(HardLinkMutation { source, destination }));
        }

        // 4. Conflicts: pick first alphabetical corpus path and deploy it
        // Same path resolution as new files - need target library from config
        for group in &data.conflicts {
            if let Some((corpus_path, _inode)) = group
                .conflicting_files
                .iter()
                .min_by(|a, b| a.0.cmp(&b.0))
            {
                let source = resolver.resolve(std::path::Path::new(corpus_path));

                // Look up target library from config
                let corpus_path_obj = std::path::Path::new(corpus_path);
                let library_name = config
                    .as_ref()
                    .and_then(|c| c.get_libraries_for_corpus_path(corpus_path_obj).first().cloned());

                let dest_rel = if let Some(lib) = library_name {
                    std::path::Path::new("libraries")
                        .join(&lib)
                        .join(&group.deploy_path)
                } else {
                    continue; // Skip if no library mapping
                };
                let destination = resolver.resolve(&dest_rel);
                mutations.push(Mutation::HardLink(HardLinkMutation { source, destination }));
            }
        }

        let count = mutations.len();
        if count == 0 {
            return 0;
        }

        // Start transaction and stage the decision
        let _ = witch.start_transaction("Deploy");
        let _ = super::super::operator_decisions::stage_decision(
            witch,
            0,
            "Deploy operations",
            mutations,
        );

        count
    }
}
