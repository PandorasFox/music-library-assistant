//! External Match action handlers.
//!
//! Handles both:
//! - The External Matches lateral view (browse/fetch/launch)
//! - The External Match Review modal (read-only browser)

use super::super::App;
use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::{external_match_modal, external_match_view, widgets, ActiveView};

impl App {
    // =========================================================================
    // External Matches Lateral View Actions
    // =========================================================================

    /// Handle actions from the External Matches lateral view.
    pub(super) fn handle_external_matches_action(
        &mut self,
        action: external_match_view::ExternalMatchesAction,
    ) {
        match action {
            external_match_view::ExternalMatchesAction::None => {}
            external_match_view::ExternalMatchesAction::CycleNext => {
                self.handle_lateral_cycle(widgets::LateralView::ExternalMatches, true);
            }
            external_match_view::ExternalMatchesAction::CyclePrev => {
                self.handle_lateral_cycle(widgets::LateralView::ExternalMatches, false);
            }
            external_match_view::ExternalMatchesAction::RequestQuit => self.handle_request_quit(),
            external_match_view::ExternalMatchesAction::RequestFetch => {
                self.witch.request_external_fetch();
                self.status_message = Some("External fetch requested".to_string());
                if let ActiveView::ExternalMatches(ref mut state) = self.view {
                    state.fetch_active = self.witch.is_external_fetch_active();
                }
            }
            external_match_view::ExternalMatchesAction::RequestReleasePacking => {
                self.witch.request_release_packing();
                self.transition_to_progress_after_mutations(
                    super::super::progress_screen::ProgressPhase::ContentAnalysis,
                );
            }
            external_match_view::ExternalMatchesAction::LaunchPackingCategory(cat) => {
                self.launch_release_packing_browser(cat);
            }
            external_match_view::ExternalMatchesAction::LaunchUntaggedReview => {
                let entries = if let ActiveView::ExternalMatches(ref state) = self.view {
                    state
                        .cached_data
                        .as_ref()
                        .map(|d| d.untagged_entries.clone())
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                self.start_external_match_review_with(entries);
            }
            external_match_view::ExternalMatchesAction::LaunchTierReview(tier) => {
                let entries = if let ActiveView::ExternalMatches(ref state) = self.view {
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
                self.start_external_match_review_with(entries);
            }
        }
    }

    /// Start external match review (read-only browser) with pre-filtered entries.
    /// Batch-loads both recording summaries and full detail data from cache.
    fn start_external_match_review_with(&mut self, entries: Vec<ExternalMatchReviewEntry>) {
        if entries.is_empty() {
            self.status_message = Some("No external matches to review".to_string());
            return;
        }

        use crate::external::musicbrainz;

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
        let ids = recording_ids;
        let (summaries, details) = self
            .cache
            .query(move |db| {
                let mut summaries = Vec::new();
                let mut details = Vec::new();

                for rec_id in &ids {
                    let rec_cache = db.get_mb_recording_cache(rec_id).ok().flatten();
                    let recording =
                        rec_cache.and_then(|(json, _)| musicbrainz::parse_recording(&json).ok());

                    let Some(rec) = recording else {
                        continue;
                    };

                    // Collect unique artist IDs from credits + relations
                    let mut artist_ids: Vec<String> = Vec::new();
                    let mut artist_seen = std::collections::HashSet::new();
                    for credit in &rec.artist_credit {
                        if artist_seen.insert(credit.artist.id.clone()) {
                            artist_ids.push(credit.artist.id.clone());
                        }
                    }
                    for relation in &rec.relations {
                        if let Some(ref artist) = relation.artist {
                            if artist_seen.insert(artist.id.clone()) {
                                artist_ids.push(artist.id.clone());
                            }
                        }
                    }

                    // Load cached artist data
                    let artists: Vec<(String, Option<musicbrainz::MbArtist>)> = artist_ids
                        .into_iter()
                        .map(|id| {
                            let parsed = db
                                .get_mb_artist_cache(&id)
                                .ok()
                                .flatten()
                                .and_then(|(json, _)| musicbrainz::parse_artist(&json).ok());
                            (id, parsed)
                        })
                        .collect();

                    // Build summary
                    let artist_credit = musicbrainz::join_artist_credits_localized(
                        &rec.artist_credit,
                        &artists,
                        &preferred_locales,
                    );

                    summaries.push((
                        rec_id.clone(),
                        external_match_modal::types::RecordingSummary {
                            title: rec.title.clone(),
                            artist_credit,
                            length_ms: rec.length.map(|l| l as u64),
                            release_count: rec.releases.len(),
                        },
                    ));

                    // Load cached release data for full detail
                    let releases: Vec<_> = rec
                        .releases
                        .iter()
                        .map(|r| {
                            let parsed = db
                                .get_mb_release_cache(&r.id)
                                .ok()
                                .flatten()
                                .and_then(|(json, _)| musicbrainz::parse_release(&json).ok());
                            (r.id.clone(), parsed)
                        })
                        .collect();

                    details.push((
                        rec_id.clone(),
                        external_match_modal::types::RecordingDetail {
                            recording: rec,
                            artists,
                            releases,
                        },
                    ));
                }

                (summaries, details)
            })
            .recv();

        let state =
            external_match_modal::ExternalMatchReviewState::new(entries, summaries, details);

        self.view = ActiveView::ExternalMatchReview(state);
    }

    // =========================================================================
    // External Match Review Modal Actions (read-only)
    // =========================================================================

    /// Handle external match review actions.
    pub(super) fn handle_external_match_review_action(
        &mut self,
        action: external_match_modal::ExternalMatchReviewAction,
        _witness: Option<&super::witness::ConfirmationGesture>,
    ) {
        match action {
            external_match_modal::ExternalMatchReviewAction::None => {}
            external_match_modal::ExternalMatchReviewAction::Cancel => {
                self.cancel_and_return_to_source("External match browser closed");
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

    // =========================================================================
    // Release Packing Browser
    // =========================================================================

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
                let prefix_owned = prefix.to_string();

                let result = self
                    .cache
                    .query(move |db| {
                        let packed = db
                            .get_packed_releases_by_category(&prefix_owned)
                            .unwrap_or_default();
                        let packing = db.get_release_packing_signal_data().unwrap_or_default();
                        let unfilled = db
                            .get_unfilled_release_slot_signal_data()
                            .unwrap_or_default();
                        let alternatives = db
                            .get_alternative_release_packing_data()
                            .unwrap_or_default();
                        let va_overrides = db
                            .get_various_artists_override_data()
                            .unwrap_or_default();
                        (packed, packing, unfilled, alternatives, va_overrides)
                    })
                    .recv();

                let (packed, packing, unfilled, alternatives, va_overrides) = result;

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
                let cat_owned = cat_str.to_string();
                let filtered = self
                    .cache
                    .query(move |db| {
                        db.get_unmatched_corpus_track_signal_data_by_category(&cat_owned)
                            .unwrap_or_default()
                    })
                    .recv();

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
            .cache
            .query(|db| db.get_packing_knots().unwrap_or_default())
            .recv();

        if knots.is_empty() {
            self.status_message = Some("No knots found".to_string());
            return;
        }

        let corpus_paths: std::collections::HashMap<i64, String> = self
            .cache
            .query(|db| {
                db.get_packing_inode_paths()
                    .unwrap_or_default()
                    .into_iter()
                    .collect()
            })
            .recv();

        let state = crate::ui::knot_browser::KnotBrowserState::build(knots, &corpus_paths);
        self.view = crate::ui::active_view::ActiveView::KnotBrowser(state);
    }

    /// Handle knot browser actions.
    pub(super) fn handle_knot_browser_action(
        &mut self,
        action: crate::ui::knot_browser::KnotBrowserAction,
    ) {
        match action {
            crate::ui::knot_browser::KnotBrowserAction::None => {}
            crate::ui::knot_browser::KnotBrowserAction::Cancel => {
                self.cancel_and_return_to_source("Knot browser closed");
            }
        }
    }

    /// Handle release packing browser actions.
    pub(super) fn handle_release_packing_browser_action(
        &mut self,
        action: crate::ui::release_packing_browser::ReleasePackingBrowserAction,
        witness: Option<&super::witness::ConfirmationGesture>,
    ) {
        match action {
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::None => {}
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::Cancel => {
                self.cancel_and_return_to_source("Release packing browser closed");
            }
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::PinRelease {
                release_id,
                track_paths,
            } => {
                if let Some(gesture) = witness {
                    self.pin_release_for_dirs(release_id, track_paths, gesture);
                }
            }
        }
    }

    /// Pin a MusicBrainz release ID for all source dirs that contain the given track paths.
    fn pin_release_for_dirs(
        &mut self,
        release_id: String,
        track_paths: Vec<String>,
        gesture: &super::witness::ConfirmationGesture,
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
            let _ = crate::ui::operator_decisions::stage_decision(
                &mut self.witch,
                key,
                &dir_label,
                vec![mutation],
                gesture,
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
}
