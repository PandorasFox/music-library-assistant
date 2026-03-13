//! External Match action handlers.
//!
//! Handles both:
//! - The External Matches lateral view (browse/fetch/launch)
//! - The External Match Review modal (read-only browser)

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::{external_match_modal, external_match_view, widgets, ActiveView};

// =========================================================================
// External Matches Lateral View Actions
// =========================================================================

impl HandleAction for external_match_view::ExternalMatchesAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            external_match_view::ExternalMatchesAction::None => {}
            external_match_view::ExternalMatchesAction::CycleNext => {
                app.handle_lateral_cycle(widgets::LateralView::ExternalMatches, true);
            }
            external_match_view::ExternalMatchesAction::CyclePrev => {
                app.handle_lateral_cycle(widgets::LateralView::ExternalMatches, false);
            }
            external_match_view::ExternalMatchesAction::RequestQuit => app.handle_request_quit(),
            external_match_view::ExternalMatchesAction::RequestFetch => {
                let _ = app.witch.request_external_fetch();
                app.status_message = Some("External fetch requested".to_string());
                if let ActiveView::ExternalMatches(ref mut state) = app.view {
                    state.fetch_active = app.witch.witch_status().is_external_fetch_active;
                }
            }
            external_match_view::ExternalMatchesAction::RequestReleasePacking => {
                let _ = app.witch.request_release_packing();
                app.transition_to_progress_after_mutations(
                    super::super::progress_screen::ProgressPhase::ContentAnalysis,
                );
            }
            external_match_view::ExternalMatchesAction::LaunchPackingCategory(cat) => {
                app.launch_release_packing_browser(cat);
            }
            external_match_view::ExternalMatchesAction::LaunchUntaggedReview => {
                let entries = if let ActiveView::ExternalMatches(ref state) = app.view {
                    state
                        .cached_data
                        .as_ref()
                        .map(|d| d.untagged_entries.clone())
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                app.start_external_match_review_with(entries);
            }
            external_match_view::ExternalMatchesAction::LaunchTierReview(tier) => {
                let entries = if let ActiveView::ExternalMatches(ref state) = app.view {
                    state
                        .cached_data
                        .as_ref()
                        .and_then(|d| {
                            d.confidence_buckets
                                .iter()
                                .find(|b| b.tier == tier)
                                .map(|b| b.entries.clone())
                        })
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                app.start_external_match_review_with(entries);
            }
        }
    }
}

// =========================================================================
// External Match Review Modal Actions (read-only)
// =========================================================================

impl HandleAction for external_match_modal::ExternalMatchReviewAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            external_match_modal::ExternalMatchReviewAction::None => {}
            external_match_modal::ExternalMatchReviewAction::Cancel => {
                app.cancel_and_return_to_source("External match browser closed");
            }
            external_match_modal::ExternalMatchReviewAction::OpenRecordingUrl(url) => {
                let _ = std::process::Command::new("xdg-open")
                    .arg(&url)
                    .stdin(std::process::Stdio::null())
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null())
                    .spawn();
            }
        }
    }
}

// =========================================================================
// Knot Browser Actions
// =========================================================================

impl HandleAction for crate::ui::knot_browser::KnotBrowserAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            crate::ui::knot_browser::KnotBrowserAction::None => {}
            crate::ui::knot_browser::KnotBrowserAction::Cancel => {
                app.cancel_and_return_to_source("Knot browser closed");
            }
        }
    }
}

// =========================================================================
// Release Packing Browser Actions
// =========================================================================

impl HandleAction for crate::ui::release_packing_browser::ReleasePackingBrowserAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::None => {}
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::Cancel => {
                app.cancel_and_return_to_source("Release packing browser closed");
            }
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::PinRelease {
                release_id,
                track_paths,
            } => {
                if let Some(gesture) = witness {
                    app.pin_release_for_dirs(release_id, track_paths, gesture);
                }
            }
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::ApproveSelected {
                selected_indices,
            } => {
                if let Some(gesture) = witness {
                    app.approve_selected_releases(selected_indices, gesture);
                }
            }
        }
    }
}

// =========================================================================
// Helper methods
// =========================================================================

impl App {
    /// Start external match review (read-only browser) with pre-filtered entries.
    /// Batch-loads both recording summaries and full detail data from cache.
    fn start_external_match_review_with(&mut self, entries: Vec<ExternalMatchReviewEntry>) {
        if entries.is_empty() {
            self.status_message = Some("No external matches to review".to_string());
            return;
        }

        // Collect unique recording IDs
        let mut recording_ids: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in &entries {
            if seen.insert(entry.recording_id.clone()) {
                recording_ids.push(entry.recording_id.clone());
            }
        }

        // Load preferred locales from config.
        let preferred_locales = crate::config::load_config()
            .map(|c| c.opinions.external_matching.preferred_locales.clone())
            .unwrap_or_default();

        // Batch query: load summaries + full detail for all recordings at once
        let batch = self
            .witch
            .query(crate::db::domain::GetRecordingBatchData {
                recording_ids,
                preferred_locales,
            });

        let state =
            external_match_modal::ExternalMatchReviewState::new(entries, batch.summaries, batch.details);

        self.view = ActiveView::ExternalMatchReview(state);
    }

    /// Load packing signal data and launch the browser for a specific category.
    fn launch_release_packing_browser(
        &mut self,
        category: crate::ui::release_packing_browser::types::PackingCategory,
    ) {
        use crate::meta::signals::data::PackedReleaseCategory;
        use crate::ui::release_packing_browser::types::PackingCategory;
        use crate::ui::release_packing_browser::ReleasePackingBrowserState;

        let state = match category {
            PackingCategory::Perfect
            | PackingCategory::FullMatches
            | PackingCategory::Singles
            | PackingCategory::Incomplete
            | PackingCategory::LowConfidence => {
                // Map UI category to signal category prefix
                let prefix = match category {
                    PackingCategory::Perfect => PackedReleaseCategory::Perfect.key_prefix(),
                    PackingCategory::FullMatches => PackedReleaseCategory::FullMatch.key_prefix(),
                    PackingCategory::Singles => PackedReleaseCategory::Single.key_prefix(),
                    PackingCategory::Incomplete => PackedReleaseCategory::Incomplete.key_prefix(),
                    PackingCategory::LowConfidence => PackedReleaseCategory::LowConfidence.key_prefix(),
                    PackingCategory::Knots
                    | PackingCategory::UnsolvedConflict
                    | PackingCategory::UnsolvedNoRelease
                    | PackingCategory::UnsolvedNoMatch => unreachable!(),
                };
                let result = self
                    .witch
                    .query(crate::db::domain::GetPackingBrowserData {
                        category_prefix: prefix.to_string(),
                    });

                let crate::db::domain::PackingBrowserData {
                    packed,
                    packing,
                    unfilled,
                    alternatives,
                    va_overrides,
                } = result;

                if packed.is_empty() {
                    self.status_message = Some("No releases in this category".to_string());
                    return;
                }

                ReleasePackingBrowserState::build_releases(
                    category,
                    packed,
                    packing,
                    unfilled,
                    alternatives,
                    va_overrides,
                )
            }
            PackingCategory::UnsolvedConflict
            | PackingCategory::UnsolvedNoRelease
            | PackingCategory::UnsolvedNoMatch => {
                use crate::meta::signals::data::UnsolvedCategory;
                let cat_str = match category {
                    PackingCategory::UnsolvedConflict => UnsolvedCategory::Conflict.as_str(),
                    PackingCategory::UnsolvedNoRelease => UnsolvedCategory::NoRelease.as_str(),
                    PackingCategory::UnsolvedNoMatch => UnsolvedCategory::NoMatch.as_str(),
                    _ => unreachable!(),
                };
                let filtered = self
                    .witch
                    .query(crate::db::domain::GetUnsolvedPackingData {
                        category: cat_str.to_string(),
                    });

                if filtered.is_empty() {
                    self.status_message = Some("No unsolved files in this category".to_string());
                    return;
                }

                ReleasePackingBrowserState::build_unmatched(category, filtered)
            }
            PackingCategory::Knots => {
                self.launch_knot_browser();
                return;
            }
        };

        self.view = ActiveView::ReleasePackingBrowser(state);
    }

    /// Load knot signal data and launch the knot browser.
    fn launch_knot_browser(&mut self) {
        let knots = self
            .witch
            .query(crate::db::domain::GetPackingKnots);

        if knots.is_empty() {
            self.status_message = Some("No knots found".to_string());
            return;
        }

        let corpus_paths: std::collections::HashMap<i64, String> = self
            .witch
            .query(crate::db::domain::GetPackingInodePaths)
            .into_iter()
            .collect();

        let state = crate::ui::knot_browser::KnotBrowserState::build(knots, &corpus_paths);
        self.view = crate::ui::active_view::ActiveView::KnotBrowser(state);
    }

    /// Pin a MusicBrainz release ID for all source dirs that contain the given track paths.
    fn pin_release_for_dirs(
        &mut self,
        release_id: String,
        track_paths: Vec<String>,
        gesture: &witness::ConfirmationGesture,
    ) {
        use crate::meta::decisions::DecisionKey;

        let config = match crate::config::load_config() {
            Ok(c) => c,
            Err(_) => {
                self.status_message = Some("Failed to load config".to_string());
                return;
            }
        };

        // Resolve unique source dirs from track paths
        let mut seen_dirs = std::collections::HashSet::new();
        let mut source_dirs: Vec<std::path::PathBuf> = Vec::new();
        for path in &track_paths {
            if let Some(resolved) = config.resolve_source_config_for_db_path(path) {
                if seen_dirs.insert(resolved.source_path.clone()) {
                    source_dirs.push(resolved.source_path);
                }
            }
        }

        if source_dirs.is_empty() {
            self.status_message = Some("No source directories found for tracks".to_string());
            return;
        }

        let label = format!("Pin release {} ({} dirs)", &release_id[..8], source_dirs.len());
        let open_txn = self.open_txn_mode();
        if !open_txn {
            let _ = self.witch.start_transaction(&label);
        }

        for source_path in &source_dirs {
            let old_dir = match config.get_raw_source_dir(source_path) {
                Some(sd) => sd.clone(),
                None => crate::config::SourceDir {
                    path: source_path.clone(),
                    libraries: vec![],
                    can_stash_dupes: None,
                    interior_dupes: None,
                    path_schema: None,
                    enable_acoustid: None,
                    pinned_release: None,
                },
            };

            let mut new_dir = old_dir.clone();
            new_dir.pinned_release = Some(release_id.clone());

            // Build the full new config with this edit applied
            let new_config = {
                let mut cfg = config.clone();
                let mut found = false;
                for sd in &mut cfg.source_dirs {
                    if sd.path == *source_path {
                        *sd = new_dir.clone();
                        found = true;
                        break;
                    }
                }
                if !found {
                    cfg.source_dirs.push(new_dir.clone());
                }
                cfg.source_dirs.retain(|sd| !sd.is_default());
                cfg
            };

            let mutation = crate::meta::mutations::Mutation::ApplyDirConfigEdit(Box::new(
                crate::meta::mutations::dir_config_edit::ApplyDirConfigEditMutation {
                    source_path: source_path.clone(),
                    old_dir,
                    new_dir,
                    new_config,
                },
            ));

            let key = DecisionKey::DirConfigEdit {
                source_path: source_path.clone(),
            };
            let dir_label = format!("Pin release: {}", source_path.display());
            let decision = gesture.decide(&dir_label, vec![mutation]);
            let _ = crate::ui::operator_decisions::stage_decision(
                &mut self.witch,
                key,
                decision,
            );
        }

        if self.open_txn_mode() {
            // Stay in the packing browser for batch pinning
            self.status_message = Some(format!(
                "Pinned {} to {} dir{}",
                &release_id[..8],
                source_dirs.len(),
                if source_dirs.len() == 1 { "" } else { "s" }
            ));
            if let ActiveView::ReleasePackingBrowser(ref mut state) = self.view {
                state.pinned_release_ids.insert(release_id);
            }
        } else {
            self.start_transaction_review();
        }
    }

    /// Approve selected releases from the packing browser: generate tag ops from MB cache.
    fn approve_selected_releases(
        &mut self,
        selected_indices: std::collections::BTreeSet<usize>,
        gesture: &witness::ConfirmationGesture,
    ) {
        use crate::external::tag_generation::{generate_tag_ops, MbTagInput};
        use crate::meta::decisions::DecisionKey;
        use crate::meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation};
        use crate::ui::release_packing_browser::SelectedReleaseData;

        // Extract release data from browser state
        let release_data: Vec<SelectedReleaseData> =
            if let ActiveView::ReleasePackingBrowser(ref state) = self.view {
                state.collect_selected_releases(&selected_indices)
            } else {
                return;
            };

        if release_data.is_empty() {
            self.status_message = Some("No releases selected".to_string());
            return;
        }

        // Load config for locales and credit routing
        let config = match crate::config::load_config() {
            Ok(c) => c,
            Err(_) => {
                self.status_message = Some("Failed to load config".to_string());
                return;
            }
        };
        let locales = config.opinions.external_matching.preferred_locales.clone();
        let routing = config.opinions.external_matching.credit_routing.clone();

        // Collect unique IDs for batch loading
        let mut release_ids: Vec<String> = Vec::new();
        let mut recording_ids: Vec<String> = Vec::new();
        let mut all_inodes: Vec<i64> = Vec::new();
        let mut seen_r = std::collections::HashSet::new();
        let mut seen_rec = std::collections::HashSet::new();

        for rd in &release_data {
            if seen_r.insert(rd.release_id.clone()) {
                release_ids.push(rd.release_id.clone());
            }
            for t in &rd.tracks {
                all_inodes.push(t.inode);
                if seen_rec.insert(t.recording_id.clone()) {
                    recording_ids.push(t.recording_id.clone());
                }
            }
        }

        // Batch query: load MB cache + current tags
        let staging = self
            .witch
            .query(crate::db::domain::GetReleaseStagingData {
                release_ids,
                recording_ids,
                inodes: all_inodes,
            });
        let bundle = staging.bundle;
        let inode_tags = staging.inode_tags;

        // Stage one decision per release
        let open_txn = self.open_txn_mode();
        if !open_txn {
            let n = release_data.len();
            let _ = self.witch.start_transaction(&format!(
                "Approve {} release{}",
                n,
                if n == 1 { "" } else { "s" }
            ));
        }

        let mut approved = 0usize;
        let mut skipped = 0usize;

        for rd in &release_data {
            let Some(release) = bundle.releases.get(&rd.release_id) else {
                skipped += rd.tracks.len();
                continue;
            };
            let total_media = release.media.len() as u32;

            let mut ops = Vec::new();
            for t in &rd.tracks {
                let Some(recording) = bundle.recordings.get(&t.recording_id) else {
                    skipped += 1;
                    continue;
                };
                let input = MbTagInput {
                    inode: t.inode,
                    recording_id: t.recording_id.clone(),
                    release_id: rd.release_id.clone(),
                    track_title: t.track_title.clone(),
                    track_position: t.track_position,
                    medium_position: t.medium_position,
                    total_media,
                    current_tags: inode_tags.get(&t.inode).cloned().unwrap_or_default(),
                };
                ops.extend(generate_tag_ops(
                    &input,
                    recording,
                    release,
                    &bundle.artists,
                    &locales,
                    &routing,
                ));
            }

            if ops.is_empty() {
                continue;
            }

            let key = DecisionKey::MbReleaseApproval {
                release_id: rd.release_id.clone(),
            };
            let label = format!(
                "Approve MB release: {}",
                &rd.release_id[..8.min(rd.release_id.len())]
            );
            let decision = gesture.decide(&label, vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                ops,
                zone: crate::db::types::Zone::Corpus,
            })]);
            let _ = crate::ui::operator_decisions::stage_decision(
                &mut self.witch,
                key,
                decision,
            );
            approved += 1;
        }

        if approved == 0 {
            self.status_message =
                Some("No releases could be approved (missing MB cache)".to_string());
            return;
        }

        self.status_message = Some(if skipped > 0 {
            format!(
                "Approved {} release{} ({} files skipped \u{2014} missing cache)",
                approved,
                if approved == 1 { "" } else { "s" },
                skipped,
            )
        } else {
            format!(
                "Approved {} release{}",
                approved,
                if approved == 1 { "" } else { "s" },
            )
        });

        self.after_staging_decisions();
    }
}
