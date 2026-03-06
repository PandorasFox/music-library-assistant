//! External Match action handlers.
//!
//! Handles both:
//! - The External Matches lateral view (browse/fetch/launch)
//! - The External Match Review modal (read-only browser)

use crate::meta::views::ExternalMatchReviewEntry;
use crate::ui::{external_match_modal, external_match_view, widgets, ActiveView};
use super::super::App;

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
                let txn = self.transactions_open();
                self.start_lateral_view(widgets::LateralView::ExternalMatches.next(txn));
            }
            external_match_view::ExternalMatchesAction::CyclePrev => {
                let txn = self.transactions_open();
                self.start_lateral_view(widgets::LateralView::ExternalMatches.prev(txn));
            }
            external_match_view::ExternalMatchesAction::RequestQuit => {
                if self.has_pending_operations() {
                    self.status_message = Some("Cannot quit while operations are pending".to_string());
                } else {
                    self.view = ActiveView::ExitConfirm(super::super::ExitConfirmModalState::default());
                }
            }
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
            external_match_view::ExternalMatchesAction::LaunchReleasePackingBrowser => {
                self.launch_release_packing_browser();
            }
            external_match_view::ExternalMatchesAction::LaunchUntaggedReview => {
                let entries = if let ActiveView::ExternalMatches(ref state) = self.view {
                    state.cached_data.as_ref()
                        .map(|d| d.untagged_entries.clone())
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                self.start_external_match_review_with(entries);
            }
            external_match_view::ExternalMatchesAction::LaunchTierReview(tier) => {
                let entries = if let ActiveView::ExternalMatches(ref state) = self.view {
                    state.cached_data.as_ref()
                        .and_then(|d| d.confidence_buckets.iter()
                            .find(|b| b.tier == tier)
                            .map(|b| b.entries.clone()))
                        .unwrap_or_default()
                } else {
                    vec![]
                };
                self.start_external_match_review_with(entries);
            }
        }
    }

    /// Start external match review (read-only browser) with pre-filtered entries.
    fn start_external_match_review_with(&mut self, entries: Vec<ExternalMatchReviewEntry>) {
        if entries.is_empty() {
            self.status_message = Some("No external matches to review".to_string());
            return;
        }

        let mut state = external_match_modal::ExternalMatchReviewState::new(entries);

        // Pre-load cached MB recording summaries for all entries
        self.preload_recording_summaries(&mut state);

        self.view = ActiveView::ExternalMatchReview(state);
    }

    /// Batch-load MB recording summaries from the cache thread.
    fn preload_recording_summaries(
        &mut self,
        state: &mut external_match_modal::ExternalMatchReviewState,
    ) {
        use crate::external::musicbrainz;

        // Collect unique recording IDs
        let mut recording_ids: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in &state.entries {
            if seen.insert(entry.recording_id.clone()) {
                recording_ids.push(entry.recording_id.clone());
            }
        }

        // Load preferred locales from config.
        let preferred_locales = crate::config::load_config()
            .map(|c| c.opinions.external_matching.preferred_locales.clone())
            .unwrap_or_default();

        // Query cache thread for all recording data in one batch
        let ids = recording_ids.clone();
        let result = self.cache.query(move |db| {
            let mut summaries = Vec::new();
            for rec_id in &ids {
                let rec_cache = db.get_mb_recording_cache(rec_id).ok().flatten();
                let recording = rec_cache
                    .and_then(|(json, _)| musicbrainz::parse_recording(&json).ok());

                if let Some(rec) = recording {
                    // Load cached artist data for locale-aware name resolution.
                    let artists: Vec<(String, Option<musicbrainz::MbArtist>)> = rec
                        .artist_credit
                        .iter()
                        .map(|c| {
                            let parsed = db
                                .get_mb_artist_cache(&c.artist.id)
                                .ok()
                                .flatten()
                                .and_then(|(json, _)| musicbrainz::parse_artist(&json).ok());
                            (c.artist.id.clone(), parsed)
                        })
                        .collect();

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
                }
            }
            summaries
        }).recv();

        for (id, summary) in result {
            state.recording_summaries.insert(id, summary);
        }
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
            external_match_modal::ExternalMatchReviewAction::ViewRecordingDetail => {
                self.load_recording_detail();
            }
            external_match_modal::ExternalMatchReviewAction::CloseRecordingDetail => {
                if let ActiveView::ExternalMatchReview(ref mut state) = self.view {
                    state.viewing_detail = None;
                }
            }
        }
    }

    /// Load MB recording detail for the current entry from cache.
    fn load_recording_detail(&mut self) {
        use crate::external::musicbrainz;

        let recording_id = match self.view {
            ActiveView::ExternalMatchReview(ref state) => {
                if state.viewing_detail.is_some() {
                    return; // Already viewing
                }
                match state.current_entry() {
                    Some(entry) => entry.recording_id.clone(),
                    None => return,
                }
            }
            _ => return,
        };

        let rec_id = recording_id.clone();
        let result = self.cache.query(move |db| {
            let rec_cache = db.get_mb_recording_cache(&rec_id).ok().flatten();
            let recording = rec_cache
                .and_then(|(json, _)| musicbrainz::parse_recording(&json).ok());

            let recording = recording?;

            // Collect unique artist IDs from credits + relations
            let mut artist_ids: Vec<String> = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for credit in &recording.artist_credit {
                if seen.insert(credit.artist.id.clone()) {
                    artist_ids.push(credit.artist.id.clone());
                }
            }
            for relation in &recording.relations {
                if let Some(ref artist) = relation.artist {
                    if seen.insert(artist.id.clone()) {
                        artist_ids.push(artist.id.clone());
                    }
                }
            }

            // Load cached artist data
            let artists: Vec<_> = artist_ids.into_iter().map(|id| {
                let parsed = db.get_mb_artist_cache(&id).ok().flatten()
                    .and_then(|(json, _)| musicbrainz::parse_artist(&json).ok());
                (id, parsed)
            }).collect();

            // Load cached release data
            let releases: Vec<_> = recording.releases.iter().map(|r| {
                let parsed = db.get_mb_release_cache(&r.id).ok().flatten()
                    .and_then(|(json, _)| musicbrainz::parse_release(&json).ok());
                (r.id.clone(), parsed)
            }).collect();

            Some((recording, artists, releases))
        }).recv();

        match result {
            Some((recording, artists, releases)) => {
                if let ActiveView::ExternalMatchReview(ref mut state) = self.view {
                    state.viewing_detail = Some(
                        external_match_modal::types::RecordingDetailState {
                            recording,
                            artists,
                            releases,
                            scroll: 0,
                        }
                    );
                }
            }
            None => {
                self.status_message = Some("No cached recording data available".to_string());
            }
        }
    }

    // =========================================================================
    // Release Packing Browser
    // =========================================================================

    /// Load packing signal data and launch the browser.
    fn launch_release_packing_browser(&mut self) {
        use crate::ui::release_packing_browser::ReleasePackingBrowserState;

        let result = self.cache.query(move |db| {
            let packing = db.get_release_packing_signal_data().unwrap_or_default();
            let unmatched = db.get_unmatched_corpus_track_signal_data().unwrap_or_default();
            let unfilled = db.get_unfilled_release_slot_signal_data().unwrap_or_default();
            let near_miss = db.get_near_miss_release_signal_data().unwrap_or_default();

            let source_key = crate::meta::external::ExternalSource::AcoustID.to_key();
            let fingerprinted_count = db.count_fingerprinted_corpus_files().unwrap_or(0);
            let matched_count = db.count_externally_matched_corpus_files(source_key).unwrap_or(0);
            let recording_count = db.count_matched_recordings(source_key).unwrap_or(0);

            (packing, unmatched, unfilled, near_miss,
             fingerprinted_count, matched_count, recording_count)
        }).recv();

        let (packing, unmatched, unfilled, near_miss,
             fingerprinted_count, matched_count, recording_count) = result;

        if packing.is_empty() && unmatched.is_empty() {
            self.status_message = Some("No release packing results available".to_string());
            return;
        }

        let state = ReleasePackingBrowserState::build(
            packing, unmatched, unfilled, near_miss,
            fingerprinted_count, matched_count, recording_count,
        );
        self.view = ActiveView::ReleasePackingBrowser(state);
    }

    /// Handle release packing browser actions.
    pub(super) fn handle_release_packing_browser_action(
        &mut self,
        action: crate::ui::release_packing_browser::ReleasePackingBrowserAction,
    ) {
        match action {
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::None => {}
            crate::ui::release_packing_browser::ReleasePackingBrowserAction::Cancel => {
                self.cancel_and_return_to_source("Release packing browser closed");
            }
        }
    }
}
