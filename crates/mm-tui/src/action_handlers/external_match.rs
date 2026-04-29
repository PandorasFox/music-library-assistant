//! External Match action handlers.
//!
//! Handles:
//! - The External Matches lateral view (browse/fetch/launch)
//! - The External Match Review modal (read-only browser)
//! - AcoustID Browse view (read-only, by confidence tier)
//! - Release Review view (multi-select approval)
//! - Knot Browser (read-only)
//! - Release Packing Browser (multi-select approval + pin)

use super::super::App;
use super::witness;
use super::HandleAction;
use crate::{external_match_view, ActiveView};

// =========================================================================
// External Matches Lateral View Actions
// =========================================================================

impl HandleAction for external_match_view::ExternalMatchesAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            external_match_view::ExternalMatchesAction::RequestFetch => {
                match app.queue_task(mm_meta::protocol::BackgroundTask::ExternalFetch) {
                    Ok(None) => {
                        app.status_message = Some("External fetch requested".to_string());
                        let fetch_active = app.witch_status().is_external_fetch_active;
                        if let ActiveView::ExternalMatches(ref mut s) = app.view {
                            s.data.fetch_active = fetch_active;
                        }
                    }
                    Ok(Some(reason)) => {
                        app.error_popup = Some(reason);
                    }
                    Err(e) => {
                        app.error_popup = Some(format!("Protocol error: {}", e));
                    }
                }
            }
            external_match_view::ExternalMatchesAction::RequestCoverArt => {
                match app.queue_task(mm_meta::protocol::BackgroundTask::CoverArtFetch) {
                    Ok(None) => {
                        app.status_message =
                            Some("Cover art fetch requested".to_string());
                        let cover_art_active =
                            app.witch_status().is_cover_art_fetch_active;
                        if let ActiveView::ExternalMatches(ref mut s) = app.view {
                            s.data.cover_art_active = cover_art_active;
                        }
                    }
                    Ok(Some(reason)) => {
                        app.error_popup = Some(reason);
                    }
                    Err(e) => {
                        app.error_popup = Some(format!("Protocol error: {}", e));
                    }
                }
            }
            external_match_view::ExternalMatchesAction::RequestReleasePacking => {
                match app.queue_task(mm_meta::protocol::BackgroundTask::ReleasePacking) {
                    Ok(Some(reason)) => {
                        app.error_popup = Some(reason);
                    }
                    _ => {
                        app.transition_to_progress_after_mutations(
                            super::super::progress_screen::ProgressPhase::ContentAnalysis,
                        );
                    }
                }
            }
            external_match_view::ExternalMatchesAction::LaunchPackingCategory(cat) => {
                use crate::release_packing_browser::types::PackingCategory;
                match cat {
                    PackingCategory::Perfect
                    | PackingCategory::FullMatches
                    | PackingCategory::Singles
                    | PackingCategory::Incomplete
                    | PackingCategory::LowConfidence => {
                        app.launch_release_review(cat);
                    }
                    PackingCategory::UnsolvedConflict
                    | PackingCategory::UnsolvedNoRelease
                    | PackingCategory::UnsolvedNoMatch
                    | PackingCategory::Knots => {
                        app.launch_release_packing_browser(cat);
                    }
                }
            }
            external_match_view::ExternalMatchesAction::LaunchUntaggedReview => {
                app.launch_acoustid_browse(mm_meta::views::external_matches::AcoustidConfidence::All);
            }
            external_match_view::ExternalMatchesAction::LaunchTierReview(tier) => {
                use mm_meta::views::ConfidenceTier;
                use mm_meta::views::external_matches::AcoustidConfidence;

                let confidence = match tier {
                    ConfidenceTier::Perfect
                    | ConfidenceTier::VeryHigh
                    | ConfidenceTier::High => AcoustidConfidence::High,
                    ConfidenceTier::Medium => AcoustidConfidence::Medium,
                    ConfidenceTier::Low => AcoustidConfidence::Low,
                };
                app.launch_acoustid_browse(confidence);
            }
        }
    }
}

// =========================================================================
// AcoustID Browse Actions
// =========================================================================

impl HandleAction for crate::acoustid_browse::AcoustidBrowseAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            crate::acoustid_browse::AcoustidBrowseAction::None => {}
            crate::acoustid_browse::AcoustidBrowseAction::Cancel => {
                app.cancel_and_return_to_source("AcoustID browse closed");
            }
            crate::acoustid_browse::AcoustidBrowseAction::OpenRecordingUrl(url) => {
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
// Release Review Actions
// =========================================================================

impl HandleAction for crate::release_review::ReleaseReviewAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            crate::release_review::ReleaseReviewAction::None => {}
            crate::release_review::ReleaseReviewAction::Cancel => {
                app.cancel_and_return_to_source("Release review closed");
            }
            crate::release_review::ReleaseReviewAction::ApproveSelected {
                selected_indices,
            } => {
                if let Some(gesture) = witness {
                    app.approve_reviewed_releases(selected_indices, gesture);
                }
            }
        }
    }
}

// =========================================================================
// Knot Browser Actions
// =========================================================================

impl HandleAction for crate::knot_browser::KnotBrowserAction {
    fn handle(self, app: &mut App, _witness: Option<&witness::ConfirmationGesture>) {
        match self {
            crate::knot_browser::KnotBrowserAction::None => {}
            crate::knot_browser::KnotBrowserAction::Cancel => {
                app.cancel_and_return_to_source("Knot browser closed");
            }
        }
    }
}

// =========================================================================
// Release Packing Browser Actions
// =========================================================================

impl HandleAction for crate::release_packing_browser::ReleasePackingBrowserAction {
    fn handle(self, app: &mut App, witness: Option<&witness::ConfirmationGesture>) {
        match self {
            crate::release_packing_browser::ReleasePackingBrowserAction::None => {}
            crate::release_packing_browser::ReleasePackingBrowserAction::Cancel => {
                app.cancel_and_return_to_source("Release packing browser closed");
            }
            crate::release_packing_browser::ReleasePackingBrowserAction::PinRelease {
                release_id,
                track_paths,
            } => {
                if let Some(gesture) = witness {
                    app.pin_release_for_dirs(release_id, track_paths, gesture);
                }
            }
            crate::release_packing_browser::ReleasePackingBrowserAction::ApproveSelected {
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
    /// Launch the AcoustID browse view, querying matches by confidence tier.
    fn launch_acoustid_browse(
        &mut self,
        confidence: mm_meta::views::external_matches::AcoustidConfidence,
    ) {
        let entries = self.query(mm_meta::domain_queries::GetAcoustidMatches { confidence });

        if entries.is_empty() {
            self.status_message = Some("No AcoustID matches for this tier".to_string());
            return;
        }

        let state = crate::acoustid_browse::AcoustidBrowseState::new(entries);
        self.view = ActiveView::AcoustidBrowse(state);
    }

    /// Launch the release review view, querying releases by packing category.
    fn launch_release_review(
        &mut self,
        category: crate::release_packing_browser::types::PackingCategory,
    ) {
        use crate::release_packing_browser::types::PackingCategory;
        use mm_meta::views::external_matches::ReleaseReviewFilter;

        let filter = match category {
            PackingCategory::Perfect => ReleaseReviewFilter::Perfect,
            PackingCategory::FullMatches => ReleaseReviewFilter::FullMatch,
            PackingCategory::Singles => ReleaseReviewFilter::Singles,
            PackingCategory::Incomplete => ReleaseReviewFilter::Incomplete,
            PackingCategory::LowConfidence => ReleaseReviewFilter::LowConfidence,
            _ => return,
        };

        let data = self.query(mm_meta::domain_queries::GetReleaseReview { filter });

        if data.releases.is_empty() {
            self.status_message = Some("No releases in this category".to_string());
            return;
        }

        let state = crate::release_review::ReleaseReviewState::new(data.releases);
        self.view = ActiveView::ReleaseReview(state);
    }

    /// Load packing signal data and launch the browser for a specific category.
    fn launch_release_packing_browser(
        &mut self,
        category: crate::release_packing_browser::types::PackingCategory,
    ) {
        use mm_meta::signals::data::PackedReleaseCategory;
        use crate::release_packing_browser::types::PackingCategory;
        use crate::release_packing_browser::ReleasePackingBrowserState;

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
                    .query(mm_meta::domain_queries::GetPackingBrowserData {
                        category_prefix: prefix.to_string(),
                    });

                let mm_meta::domain_query_types::PackingBrowserData {
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
                use mm_meta::signals::data::UnsolvedCategory;
                let cat_str = match category {
                    PackingCategory::UnsolvedConflict => UnsolvedCategory::Conflict.as_str(),
                    PackingCategory::UnsolvedNoRelease => UnsolvedCategory::NoRelease.as_str(),
                    PackingCategory::UnsolvedNoMatch => UnsolvedCategory::NoMatch.as_str(),
                    _ => unreachable!(),
                };
                let filtered = self
                    .query(mm_meta::domain_queries::GetUnsolvedPackingData {
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
            .query(mm_meta::domain_queries::GetPackingKnots);

        if knots.is_empty() {
            self.status_message = Some("No knots found".to_string());
            return;
        }

        let corpus_paths: std::collections::HashMap<i64, String> = self
            .query(mm_meta::domain_queries::GetPackingInodePaths)
            .into_iter()
            .collect();

        let state = crate::knot_browser::KnotBrowserState::build(knots, &corpus_paths);
        self.view = crate::active_view::ActiveView::KnotBrowser(state);
    }

    /// Pin a MusicBrainz release ID for all source dirs that contain the given track paths.
    fn pin_release_for_dirs(
        &mut self,
        release_id: String,
        track_paths: Vec<String>,
        gesture: &witness::ConfirmationGesture,
    ) {
        let config = self.config();

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
            let _ = self.start_transaction(&label);
        }

        for source_path in &source_dirs {
            let old_dir = match config.get_raw_source_dir(source_path) {
                Some(sd) => sd.clone(),
                None => mm_meta::config::SourceDir {
                    path: source_path.clone(),
                    libraries: vec![],
                    can_stash_dupes: None,
                    interior_dupes: None,
                    path_schema: None,
                    enable_acoustid: None,
                    pinned_release: None,
                    cover_art_sanctity: None,
                },
            };

            let mut new_dir = old_dir.clone();
            new_dir.pinned_release = Some(release_id.clone());

            // Build the full new config with this edit applied
            let new_config = {
                let mut cfg = (*config).clone();
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

            let mutation = mm_meta::mutations::Mutation::ApplyDirConfigEdit(Box::new(
                mm_meta::mutations::dir_config_edit::ApplyDirConfigEditMutation {
                    source_path: source_path.clone(),
                    old_dir,
                    new_dir,
                    new_config,
                },
            ));

            let key = mm_ui::decision_keys::dir_config_edit(source_path.clone());
            let dir_label = format!("Pin release: {}", source_path.display());
            let decision = gesture.decide(&dir_label, vec![mutation]);
            let _ = crate::operator_decisions::stage_decision(
                self,
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
        use mm_meta::views::external_matches::{ReleaseApprovalInput, ApprovalTrackInput};

        // Extract release data from browser state -> shared approval inputs
        let approval_inputs: Vec<ReleaseApprovalInput> =
            if let ActiveView::ReleasePackingBrowser(ref state) = self.view {
                state.collect_selected_releases(&selected_indices)
                    .into_iter()
                    .map(|rd| ReleaseApprovalInput {
                        release_id: rd.release_id,
                        tracks: rd.tracks.into_iter().map(|t| ApprovalTrackInput {
                            inode: t.inode,
                            recording_id: t.recording_id,
                            track_title: t.track_title,
                            track_position: t.track_position,
                            medium_position: t.medium_position,
                        }).collect(),
                    })
                    .collect()
            } else {
                return;
            };

        self.run_approval(approval_inputs, gesture);
    }

    /// Approve selected releases from the release review view.
    fn approve_reviewed_releases(
        &mut self,
        selected_indices: std::collections::BTreeSet<usize>,
        gesture: &witness::ConfirmationGesture,
    ) {
        use mm_meta::views::external_matches::{ReleaseApprovalInput, ApprovalTrackInput};

        let approval_inputs: Vec<ReleaseApprovalInput> =
            if let ActiveView::ReleaseReview(ref state) = self.view {
                selected_indices
                    .iter()
                    .filter_map(|&idx| {
                        let entry = state.entries.get(idx)?;
                        let release = &entry.release;
                        let tracks: Vec<ApprovalTrackInput> = release
                            .tracks
                            .iter()
                            .filter_map(|t| {
                                let inode = t.matched_inode?;
                                Some(ApprovalTrackInput {
                                    inode,
                                    recording_id: t.recording_id.clone(),
                                    track_title: t.mb_title.clone(),
                                    track_position: t.position as u32,
                                    medium_position: t.medium_position,
                                })
                            })
                            .collect();
                        if tracks.is_empty() {
                            return None;
                        }
                        Some(ReleaseApprovalInput {
                            release_id: release.release_id.clone(),
                            tracks,
                        })
                    })
                    .collect()
            } else {
                return;
            };

        self.run_approval(approval_inputs, gesture);
    }

    /// Shared approval logic.
    ///
    /// Closed-txn mode (default): single protocol round-trip via
    /// `BatchApproveReleases`. The witch loads staging data, builds
    /// decisions, discards any open txn, opens a fresh one, and stages
    /// all decisions in-process.
    ///
    /// Open-txn mode: falls back to the per-decision flow so that
    /// decisions append to the existing persistent transaction (the
    /// batched endpoint always discards). Used for composite flows
    /// where the operator is building up a multi-source transaction.
    fn run_approval(
        &mut self,
        approval_inputs: Vec<mm_meta::views::external_matches::ReleaseApprovalInput>,
        gesture: &witness::ConfirmationGesture,
    ) {
        if approval_inputs.is_empty() {
            self.status_message = Some("No releases selected".to_string());
            return;
        }

        if self.open_txn_mode() {
            self.run_approval_per_decision(approval_inputs, gesture);
            return;
        }

        // Closed-txn fast path: single round-trip.
        let release_ids: Vec<String> = approval_inputs
            .iter()
            .map(|r| r.release_id.clone())
            .collect();
        // Gesture has already authorized this approval via the action
        // handler entry point; the protocol message itself doesn't carry
        // a gesture (decisions are constructed server-side).
        let _ = gesture;
        match self.batch_approve_releases(release_ids) {
            Ok((staged, skipped)) => {
                self.status_message = Some(format_approval_message(staged, skipped));
                self.after_staging_decisions();
            }
            Err(e) => {
                self.status_message = Some(format!("Approval failed: {e}"));
            }
        }
    }

    /// Per-decision approval flow. Used in open-txn mode where decisions
    /// must append to an existing transaction without discarding it.
    fn run_approval_per_decision(
        &mut self,
        approval_inputs: Vec<mm_meta::views::external_matches::ReleaseApprovalInput>,
        gesture: &witness::ConfirmationGesture,
    ) {
        use mm_meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation};
        use mm_ui::external_matches::approval::build_release_approval_decisions;

        // Load config for locales and credit routing
        let config = self.config();
        let locales = config.opinions.external_matching.preferred_locales.clone();
        let routing = config.opinions.external_matching.credit_routing.clone();
        let tag_names = config.opinions.external_matching.mb_tag_names.clone();

        // Collect unique IDs for batch loading
        let mut release_ids: Vec<String> = Vec::new();
        let mut recording_ids: Vec<String> = Vec::new();
        let mut all_inodes: Vec<i64> = Vec::new();
        let mut seen_r = std::collections::HashSet::new();
        let mut seen_rec = std::collections::HashSet::new();

        for rd in &approval_inputs {
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
            .query(mm_meta::domain_queries::GetReleaseStagingData {
                release_ids,
                recording_ids,
                inodes: all_inodes,
            });

        // Use shared approval builder (same logic for TUI and web)
        let (decisions, skipped) = build_release_approval_decisions(
            &approval_inputs,
            &staging.bundle,
            &staging.inode_tags,
            &locales,
            &routing,
            &tag_names,
        );

        if decisions.is_empty() {
            self.status_message =
                Some("No releases could be approved (missing MB cache)".to_string());
            return;
        }

        let approved = decisions.len();
        for ad in decisions {
            let key = mm_ui::decision_keys::mb_release_approval(ad.release_id);
            let mutations: Vec<Mutation> = ad
                .per_inode_ops
                .into_iter()
                .map(|ops| {
                    Mutation::ApplyTagOps(ApplyTagOpsMutation {
                        ops,
                        zone: mm_meta::db_types::Zone::Corpus,
                    })
                })
                .collect();
            let decision = gesture.decide(&ad.label, mutations);
            let _ = crate::operator_decisions::stage_decision(
                self,
                key,
                decision,
            );
        }

        self.status_message = Some(format_approval_message(approved, skipped));
        self.after_staging_decisions();
    }
}

fn format_approval_message(approved: usize, skipped: usize) -> String {
    if skipped > 0 {
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
    }
}
