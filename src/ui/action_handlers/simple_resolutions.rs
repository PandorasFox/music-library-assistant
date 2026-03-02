//! Simple Resolution Modals
//!
//! Handles missing file, missing directory, corrupt file, shit format,
//! subpar duplicate, and directory overlap resolution modals.
//! These flows share a common pattern: load data, show preview, stage mutations.

use crate::meta::decisions::DecisionKey;
use crate::ui::{corrupt_file_modal, embed_album_art_modal, missing_directory_modal, missing_file_modal, shit_format_modal, subpar_duplicate_modal, ActiveView};
use super::witness;
use super::super::App;

impl App {
    // =========================================================================
    // Missing File Resolution
    // =========================================================================

    /// Start missing file resolution modal from Insights view.
    pub(in crate::ui) fn start_missing_file_resolution(&mut self) {
        // Load categorized missing file data
        let data = self.cache.query(|db| {
            missing_file_modal::MissingFileModalData::load(&db).ok().unwrap_or_default()
        }).recv();

        // Create preview state with cached data
        let preview = missing_file_modal::MissingFilePreviewState::new(data);
        self.view = ActiveView::MissingFileResolution(preview);
    }

    /// Handle missing file preview actions.
    pub(super) fn handle_missing_file_preview_action(&mut self, action: missing_file_modal::MissingFilePreviewAction, witness: Option<&witness::ConfirmationGesture>) {
        match action {
            missing_file_modal::MissingFilePreviewAction::None => {}
            missing_file_modal::MissingFilePreviewAction::ConfirmRestore => {
                let Some(w) = witness else { return };
                // Generate restore mutations (HardLink) and stage for review
                let mutations = match &self.view {
                    ActiveView::MissingFileResolution(ref preview) => preview.cached_data.restore_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Restore missing files", DecisionKey::MissingFile, w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No files to restore".to_string());
                }
            }
            missing_file_modal::MissingFilePreviewAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                // Generate drop mutations for ALL missing files (restorable + non-restorable)
                let mutations = match &self.view {
                    ActiveView::MissingFileResolution(ref preview) => preview.cached_data.drop_all_missing(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Drop missing files", DecisionKey::MissingFile, w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No files to drop".to_string());
                }
            }
            missing_file_modal::MissingFilePreviewAction::Cancel => {
                self.cancel_and_return_to_source("Missing file resolution cancelled");
            }
        }
    }

    // =========================================================================
    // Missing Directory Resolution
    // =========================================================================

    /// Start missing directory resolution modal from Insights view.
    pub(in crate::ui) fn start_missing_directory_resolution(&mut self) {
        // Load missing directory data
        let data = self.cache.query(|db| {
            missing_directory_modal::MissingDirectoryModalData::load(&db).ok().unwrap_or_default()
        }).recv();

        // Create preview state with cached data
        let preview = missing_directory_modal::MissingDirectoryPreviewState::new(data);
        self.view = ActiveView::MissingDirectoryResolution(preview);
    }

    /// Handle missing directory preview actions.
    pub(super) fn handle_missing_directory_preview_action(&mut self, action: missing_directory_modal::MissingDirectoryPreviewAction, witness: Option<&witness::ConfirmationGesture>) {
        match action {
            missing_directory_modal::MissingDirectoryPreviewAction::None => {}
            missing_directory_modal::MissingDirectoryPreviewAction::ConfirmDrop => {
                let Some(w) = witness else { return };
                // Generate drop mutations (DropDirectoryFromIndex) and stage for review
                let mutations = match &self.view {
                    ActiveView::MissingDirectoryResolution(ref preview) => preview.cached_data.drop_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Drop missing directories", DecisionKey::MissingDirectory, w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No directories to drop".to_string());
                }
            }
            missing_directory_modal::MissingDirectoryPreviewAction::Cancel => {
                self.cancel_and_return_to_source("Missing directory resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Corrupt File Resolution
    // ========================================================================

    /// Start corrupt file resolution modal from Insights view.
    pub(in crate::ui) fn start_corrupt_file_resolution(&mut self) {
        // Load corrupt file data
        let data = self.cache.query(|db| {
            corrupt_file_modal::CorruptFileModalData::load(&db).ok().unwrap_or_default()
        }).recv();

        // Create preview state with cached data
        let preview = corrupt_file_modal::CorruptFilePreviewState::new(data);
        self.view = ActiveView::CorruptFileResolution(preview);
    }

    /// Handle corrupt file preview actions.
    pub(super) fn handle_corrupt_file_preview_action(&mut self, action: corrupt_file_modal::CorruptFilePreviewAction, witness: Option<&witness::ConfirmationGesture>) {
        match action {
            corrupt_file_modal::CorruptFilePreviewAction::None => {}
            corrupt_file_modal::CorruptFilePreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                // Generate stash + drop mutations and stage for review
                let mutations = match &self.view {
                    ActiveView::CorruptFileResolution(ref preview) => preview.cached_data.stash_and_drop_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Stash corrupt files", DecisionKey::CorruptFile, w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No files to stash".to_string());
                }
            }
            corrupt_file_modal::CorruptFilePreviewAction::Cancel => {
                self.cancel_and_return_to_source("Corrupt file resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Shit Format Resolution
    // ========================================================================

    /// Start shit format resolution modal from Insights view.
    pub(in crate::ui) fn start_shit_format_resolution(&mut self) {
        // Load shit format file data
        let mut data = self.cache.query(|db| {
            shit_format_modal::ShitFormatModalData::load(&db).ok().unwrap_or_default()
        }).recv();

        // Thread opinion: lossy files -> FLAC capture instead of Opus transcode
        data.lossy_to_flac = self.config().opinions.lossy_shit_formats_to_flac;

        // Create preview state with cached data
        let preview = shit_format_modal::ShitFormatPreviewState::new(data);
        self.view = ActiveView::ShitFormatResolution(preview);
    }

    /// Handle shit format preview actions.
    pub(super) fn handle_shit_format_preview_action(&mut self, action: shit_format_modal::ShitFormatPreviewAction, witness: Option<&witness::ConfirmationGesture>) {
        match action {
            shit_format_modal::ShitFormatPreviewAction::None => {}
            shit_format_modal::ShitFormatPreviewAction::ConfirmRemuxLossless => {
                let Some(w) = witness else { return };
                // Generate FLAC remux mutations for lossless files only
                let mutations = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => preview.cached_data.lossless_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Remux to FLAC", DecisionKey::ShitFormat, w);
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No lossless files to remux".to_string());
                }
            }
            shit_format_modal::ShitFormatPreviewAction::ConfirmTranscodeLossy => {
                let Some(w) = witness else { return };
                let (mutations, lossy_to_flac) = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => {
                        (preview.cached_data.lossy_mutations(), preview.cached_data.lossy_to_flac)
                    }
                    _ => (Vec::new(), false),
                };
                if !mutations.is_empty() {
                    let label = if lossy_to_flac { "Capture lossy to FLAC" } else { "Transcode to Opus" };
                    self.stage_mutations_with_transaction(mutations, label, DecisionKey::ShitFormat, w);
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No lossy files to transcode".to_string());
                }
            }
            shit_format_modal::ShitFormatPreviewAction::ConfirmConvertAll => {
                let Some(w) = witness else { return };
                let (mutations, lossy_to_flac) = match &self.view {
                    ActiveView::ShitFormatResolution(ref preview) => {
                        (preview.cached_data.all_mutations(), preview.cached_data.lossy_to_flac)
                    }
                    _ => (Vec::new(), false),
                };
                if !mutations.is_empty() {
                    let label = if lossy_to_flac { "Remux and capture all to FLAC" } else { "Convert all formats" };
                    self.stage_mutations_with_transaction(mutations, label, DecisionKey::ShitFormat, w);
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No files to convert".to_string());
                }
            }
            shit_format_modal::ShitFormatPreviewAction::Cancel => {
                self.cancel_and_return_to_source("Shit format resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Subpar Duplicate Resolution
    // ========================================================================

    /// Start subpar duplicate resolution modal from Insights view.
    pub(in crate::ui) fn start_subpar_duplicate_resolution(&mut self) {
        // Load subpar duplicate file data
        let data = self.cache.query(|db| {
            subpar_duplicate_modal::SubparDuplicateModalData::load(&db).ok().unwrap_or_default()
        }).recv();

        // Create preview state with cached data
        let preview = subpar_duplicate_modal::SubparDuplicatePreviewState::new(data);
        self.view = ActiveView::SubparDuplicateResolution(preview);
    }

    /// Handle subpar duplicate preview actions.
    pub(super) fn handle_subpar_duplicate_preview_action(&mut self, action: subpar_duplicate_modal::SubparDuplicatePreviewAction, witness: Option<&witness::ConfirmationGesture>) {
        match action {
            subpar_duplicate_modal::SubparDuplicatePreviewAction::None => {}
            subpar_duplicate_modal::SubparDuplicatePreviewAction::ConfirmStashAll => {
                let Some(w) = witness else { return };
                // Generate stash + drop mutations and stage for review
                let mutations = match &self.view {
                    ActiveView::SubparDuplicateResolution(ref preview) => preview.cached_data.stash_and_drop_mutations(),
                    _ => Vec::new(),
                };
                if !mutations.is_empty() {
                    self.stage_mutations_with_transaction(mutations, "Stash subpar duplicates", DecisionKey::SubparDuplicate, w);
                    // Note: view is NOT reset here - preserved for Cancel return via TransactionReview
                    self.after_staging_decisions();
                } else {
                    self.status_message = Some("No files to stash".to_string());
                }
            }
            subpar_duplicate_modal::SubparDuplicatePreviewAction::Cancel => {
                self.cancel_and_return_to_source("Subpar duplicate resolution cancelled");
            }
        }
    }

    // ========================================================================
    // Directory Overlap Cluster Resolution
    // ========================================================================

    /// Start directory overlap cluster resolution modal from Insights view.
    pub(in crate::ui) fn start_directory_overlap_resolution(&mut self) {
        use super::super::directory_cluster_modal;

        // Load directory overlap cluster data
        let data = self.cache.query(|db| {
            directory_cluster_modal::DirectoryClusterModalData::load(&db).ok().unwrap_or_default()
        }).recv();

        // Create preview state with cached data
        let preview = directory_cluster_modal::DirectoryClusterPreviewState::new(data);
        self.view = ActiveView::DirectoryClusterResolution(preview);
    }

    /// Handle directory cluster preview actions.
    pub(super) fn handle_directory_cluster_preview_action(
        &mut self,
        action: super::super::directory_cluster_modal::DirectoryClusterPreviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        use super::super::directory_cluster_modal::DirectoryClusterPreviewAction;

        match action {
            DirectoryClusterPreviewAction::None => {}
            DirectoryClusterPreviewAction::ConfirmCurrent => {
                let Some(w) = witness else { return };

                // Check if selected option is MarkExpected — handle via dedicated path
                let is_mark_expected = matches!(
                    &self.view,
                    ActiveView::DirectoryClusterResolution(ref preview)
                        if matches!(preview.selected_option(), Some(crate::ui::directory_cluster_modal::types::ClusterResolutionOption::MarkExpected))
                );

                if is_mark_expected {
                    // Delegate to MarkExpected handler
                    self.handle_directory_cluster_preview_action(
                        DirectoryClusterPreviewAction::MarkExpected,
                        Some(w),
                    );
                    return;
                }

                // Check if selected option is EditTags — launch tag editor
                let edit_tags_inodes = match &self.view {
                    ActiveView::DirectoryClusterResolution(ref preview) => {
                        match preview.selected_option() {
                            Some(crate::ui::directory_cluster_modal::types::ClusterResolutionOption::EditTags { inodes, .. }) => {
                                Some(inodes.clone())
                            }
                            _ => None,
                        }
                    }
                    _ => None,
                };

                if let Some(inodes) = edit_tags_inodes {
                    if !inodes.is_empty() {
                        let audio_files = self.cache.query(move |db| {
                            db.get_audio_files_by_inodes(
                                &inodes,
                                crate::db::types::Zone::Corpus,
                            ).unwrap_or_default()
                        }).recv();
                        if !audio_files.is_empty() {
                            self.open_unified_tag_editor_bulk(
                                audio_files,
                                super::super::tag_editor::TagEditorSource::HealthModal,
                                None,
                            );
                        }
                    }
                    return;
                }

                // Stage mutations for current cluster's selected option and advance
                let (cluster_index, mutations) = match &self.view {
                    ActiveView::DirectoryClusterResolution(ref preview) => {
                        if let Some(option) = preview.selected_option() {
                            let mutations = preview.cached_data.mutations_for_resolution(
                                preview.current_cluster_index,
                                option,
                            );
                            (preview.current_cluster_index, mutations)
                        } else {
                            (0, Vec::new())
                        }
                    }
                    _ => (0, Vec::new()),
                };
                if !mutations.is_empty() {
                    self.stage_directory_cluster_mutations(cluster_index, mutations, "Resolve directory overlap", w);
                }
                // Navigate to next cluster
                let at_last = if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                    !preview.navigate_next()
                } else {
                    true
                };
                if at_last {
                    // Last cluster - go to review
                    self.after_staging_decisions();
                }
            }
            DirectoryClusterPreviewAction::MarkExpected => {
                let Some(w) = witness else { return };
                // Extract source_a/source_b from current cluster's key and stage EmitExpectedOverlap
                let mutation = match &self.view {
                    ActiveView::DirectoryClusterResolution(ref preview) => {
                        preview.current_cluster().map(|cluster| {
                            let parts: Vec<&str> = cluster.cluster_key.splitn(2, '|').collect();
                            let source_a = parts.first().unwrap_or(&"").to_string();
                            let source_b = parts.get(1).unwrap_or(&"").to_string();
                            crate::meta::mutations::Mutation::EmitExpectedOverlap(
                                crate::meta::mutations::indexing::EmitExpectedOverlapMutation {
                                    source_a,
                                    source_b,
                                },
                            )
                        })
                    }
                    _ => None,
                };
                if let Some(mutation) = mutation {
                    self.stage_directory_cluster_mutations(
                        match &self.view {
                            ActiveView::DirectoryClusterResolution(ref preview) => preview.current_cluster_index,
                            _ => 0,
                        },
                        vec![mutation],
                        "Mark expected overlap",
                        w,
                    );
                }
                // Navigate to next cluster
                let at_last = if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                    !preview.navigate_next()
                } else {
                    true
                };
                if at_last {
                    self.after_staging_decisions();
                }
            }
            DirectoryClusterPreviewAction::NavigateNext => {
                if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                    preview.navigate_next();
                }
            }
            DirectoryClusterPreviewAction::NavigatePrev => {
                if let ActiveView::DirectoryClusterResolution(ref mut preview) = self.view {
                    preview.navigate_prev();
                }
            }
            DirectoryClusterPreviewAction::ShowReview => {
                // Jump directly to transaction review
                self.after_staging_decisions();
            }
            DirectoryClusterPreviewAction::Cancel => {
                self.cancel_and_return_to_source("Directory overlap cluster resolution cancelled");
            }
        }
    }

    // =========================================================================
    // Release Overlap Resolution
    // =========================================================================

    /// Start release overlap resolution modal from Insights view.
    ///
    /// Loads ReleaseOverlapSignal data into DirectoryClusterModalData and
    /// reuses the DirectoryClusterResolution view.
    pub(in crate::ui) fn start_release_overlap_resolution(&mut self) {
        use super::super::directory_cluster_modal;

        // Load release overlap data
        let data = self.cache.query(|db| {
            directory_cluster_modal::DirectoryClusterModalData::load_release_overlaps(&db).ok().unwrap_or_default()
        }).recv();

        // Create preview state with cached data
        let preview = directory_cluster_modal::DirectoryClusterPreviewState::new(data);
        self.view = ActiveView::DirectoryClusterResolution(preview);
    }

    // =========================================================================
    // Album Art Review
    // =========================================================================

    /// Start per-directory album art review modal from Insights view.
    pub(in crate::ui) fn start_album_art_review(&mut self) {
        let directories = self.cache.query(|db| {
            embed_album_art_modal::load_review_directories(&db).unwrap_or_default()
        }).recv();

        // Preload all art images upfront to avoid per-frame jank during navigation
        {
            use crate::corpus::paths;
            use crate::ui::widgets::PreloadEntry;

            let resolver = paths::get_resolver();
            let mut preload = Vec::new();
            for dir in &directories {
                preload.push(PreloadEntry::Sidecar(
                    std::path::PathBuf::from(&dir.sidecar.path),
                ));
                for entry in &dir.upgrade_entries {
                    let audio_path = resolver.resolve(std::path::Path::new(&entry.path));
                    preload.push(PreloadEntry::Embedded(audio_path));
                }
            }
            self.art_cache.preload_set(preload, &mut self.art_picker);
        }

        let state = embed_album_art_modal::AlbumArtReviewState::new(directories);
        self.view = ActiveView::EmbedAlbumArtResolution(state);
    }

    /// Handle album art review actions (per-directory replace/append/skip/cancel).
    pub(super) fn handle_album_art_review_action(
        &mut self,
        action: embed_album_art_modal::AlbumArtReviewAction,
        witness: Option<&witness::ConfirmationGesture>,
    ) {
        match action {
            embed_album_art_modal::AlbumArtReviewAction::None => {}
            embed_album_art_modal::AlbumArtReviewAction::ReplaceDirectory => {
                let Some(w) = witness else { return };
                self.confirm_album_art_directory(embed_album_art_modal::AlbumArtReviewButton::Replace, w);
            }
            embed_album_art_modal::AlbumArtReviewAction::AppendDirectory => {
                let Some(w) = witness else { return };
                self.confirm_album_art_directory(embed_album_art_modal::AlbumArtReviewButton::Append, w);
            }
            embed_album_art_modal::AlbumArtReviewAction::SkipDirectory => {
                let Some(w) = witness else { return };
                if let ActiveView::EmbedAlbumArtResolution(ref mut state) = self.view {
                    let all_done = state.mark_processed_and_advance();
                    if all_done {
                        self.finalize_album_art_review(w);
                    }
                }
            }
            embed_album_art_modal::AlbumArtReviewAction::Cancel => {
                self.cancel_and_return_to_source("Album art review cancelled");
            }
        }
    }

    /// Confirm current directory with the given button mode (Replace or Append).
    fn confirm_album_art_directory(
        &mut self,
        button: embed_album_art_modal::AlbumArtReviewButton,
        w: &witness::ConfirmationGesture,
    ) {
        if let ActiveView::EmbedAlbumArtResolution(ref mut state) = self.view {
            if let Some(dir) = state.directories.get(state.current_dir) {
                let mutations = dir.mutations_with_mode(button);
                state.staged_mutations.extend(mutations);
                state.confirmed_count += 1;
            }
            let all_done = state.mark_processed_and_advance();
            if all_done {
                self.finalize_album_art_review(w);
            }
        }
    }

    /// All directories processed — stage accumulated mutations for review.
    fn finalize_album_art_review(&mut self, w: &witness::ConfirmationGesture) {
        if let ActiveView::EmbedAlbumArtResolution(ref mut state) = self.view {
            let mutations = std::mem::take(&mut state.staged_mutations);
            if !mutations.is_empty() {
                self.stage_mutations_with_transaction(mutations, "Album art", DecisionKey::AlbumArt, w);
                self.after_staging_decisions();
            } else {
                self.cancel_and_return_to_source("No album art changes staged");
            }
        }
    }

    /// Stage directory cluster mutations for transaction review.
    fn stage_directory_cluster_mutations(&mut self, cluster_index: usize, mutations: Vec<crate::meta::mutations::Mutation>, label: &str, gesture: &witness::ConfirmationGesture) {
        // Start transaction if not already started
        if !self.witch.has_transaction() {
            let _ = self.witch.start_transaction("Directory overlap resolution");
        }
        let _ = super::super::operator_decisions::stage_decision(
            &mut self.witch,
            DecisionKey::DirectoryCluster { cluster_index },
            label,
            mutations,
            gesture,
        );
    }
}
