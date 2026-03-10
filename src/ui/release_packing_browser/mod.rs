//! Release Packing Browser — read-only full-screen browser for packing results.
//!
//! Three-pane layout:
//! - Left: flat list of releases, near-misses, and unmatched files
//! - Middle: tracks/slots for the selected release
//! - Bottom-right: per-track detail (score breakdown, file info)

pub mod render;
pub mod types;

use std::collections::{HashMap, HashSet};

use crate::meta::signals::data::{
    AlternativeReleasePackingData, PackedReleaseData, ReleasePackingData, UnfilledReleaseSlotData,
    UnmatchedCorpusTrackData, VariousArtistsOverrideData, VariousArtistsOverrideSource,
};
use crate::ui::input::InputAction;
use crate::ui::widgets::TextInputState;
use crate::ui::widgets::ListClickTargets;
use types::*;

// ============================================================================
// Actions
// ============================================================================

pub(crate) enum ReleasePackingBrowserAction {
    None,
    Cancel,
    PinRelease {
        release_id: String,
        track_paths: Vec<String>,
    },
}

// ============================================================================
// State
// ============================================================================

pub(crate) struct ReleasePackingBrowserState {
    // Which category this browser is showing
    pub category: PackingCategory,

    // Left pane (flat list)
    pub entries: Vec<PackingListEntry>,
    pub cursor: usize,
    pub scroll: usize,

    // Middle pane (tracks for selected release)
    pub track_cursor: usize,
    pub track_scroll: usize,

    // Detail pane
    pub detail_scroll: usize,

    // Focus
    pub focused_pane: FocusedPane,

    // Click targets
    pub click_targets: ListClickTargets,

    // Pin release input overlay
    pub pin_input: Option<TextInputState>,
    pub pin_error: Option<String>,

    // Release IDs pinned during this browser session (for visual feedback)
    pub pinned_release_ids: HashSet<String>,

    // Source data (only the category being viewed is populated)
    pub releases: Vec<ReleaseGroup>,
    pub unmatched: Vec<UnmatchedEntry>,
}

// ============================================================================
// Construction
// ============================================================================

impl ReleasePackingBrowserState {
    /// Build browser state for a release category (FullMatches, Singles, Incomplete)
    /// from PackedReleaseData signals + per-inode track data + unfilled slots.
    pub fn build_releases(
        category: PackingCategory,
        packed: Vec<PackedReleaseData>,
        packing_rows: Vec<(i64, String, ReleasePackingData)>,
        unfilled_rows: Vec<UnfilledReleaseSlotData>,
        alt_data: Vec<AlternativeReleasePackingData>,
        va_data: Vec<VariousArtistsOverrideData>,
    ) -> Self {
        // Group packing rows by release_id
        let mut track_map: HashMap<String, Vec<(i64, String, ReleasePackingData)>> = HashMap::new();
        for row in packing_rows {
            track_map
                .entry(row.2.release_id.clone())
                .or_default()
                .push(row);
        }

        // Group unfilled slots by release_id
        let mut unfilled_map: HashMap<String, Vec<UnfilledReleaseSlotData>> = HashMap::new();
        for slot in unfilled_rows {
            unfilled_map
                .entry(slot.release_id.clone())
                .or_default()
                .push(slot);
        }

        // Group alternatives by winner release_id
        let mut alt_map: HashMap<String, Vec<AlternativeReleaseInfo>> = HashMap::new();
        for alt in alt_data {
            alt_map
                .entry(alt.winner_release_id.clone())
                .or_default()
                .push(AlternativeReleaseInfo {
                    release_id: alt.alternative_release_id,
                    release_title: alt.alternative_release_title,
                    release_artist: alt.alternative_release_artist,
                    alternative_score: alt.alternative_score,
                    winner_score: alt.winner_score,
                    inode_count: alt.inode_count,
                });
        }

        // Index VA overrides by release_id
        let mut va_map: HashMap<String, VaOverrideInfo> = HashMap::new();
        for va in va_data {
            va_map.insert(
                va.release_id,
                VaOverrideInfo {
                    suggested_artist: va.suggested_artist,
                    source: match va.source {
                        VariousArtistsOverrideSource::ExactAlternative => {
                            "exact alternative".to_string()
                        }
                        VariousArtistsOverrideSource::CompetingProposal => {
                            "competing proposal".to_string()
                        }
                    },
                },
            );
        }

        // Build release groups from PackedReleaseData (already categorized)
        let mut releases: Vec<ReleaseGroup> = Vec::new();

        for pr in packed {
            let mut tracks: Vec<AssignedTrackInfo> = track_map
                .remove(&pr.release_id)
                .unwrap_or_default()
                .into_iter()
                .map(|(_inode, path, data)| AssignedTrackInfo {
                    path,
                    track_number: data.track_number.clone(),
                    track_title: data.track_title.clone(),
                    medium_position: data.medium_position,
                    track_position: data.track_position,
                    medium_format: data.medium_format.clone(),
                    recording_id: data.recording_id.clone(),
                    score: data.score,
                    score_breakdown: data.score_breakdown.clone(),
                    alternatives_count: data.alternatives_count,
                })
                .collect();
            tracks.sort_by(|a, b| {
                a.medium_position
                    .cmp(&b.medium_position)
                    .then(a.track_position.cmp(&b.track_position))
            });

            let mut unfilled: Vec<UnfilledSlotInfo> = unfilled_map
                .remove(&pr.release_id)
                .unwrap_or_default()
                .into_iter()
                .map(|s| UnfilledSlotInfo {
                    medium_pos: s.medium_pos,
                    track_pos: s.track_pos,
                    track_title: s.track_title,
                    recording_id: s.recording_id,
                })
                .collect();
            unfilled.sort_by(|a, b| {
                a.medium_pos
                    .cmp(&b.medium_pos)
                    .then(a.track_pos.cmp(&b.track_pos))
            });

            let coverage = if pr.total_tracks > 0 {
                pr.assigned_count as f32 / pr.total_tracks as f32
            } else {
                0.0
            };

            let mut alternatives = alt_map.remove(&pr.release_id).unwrap_or_default();
            alternatives.sort_by(|a, b| {
                b.alternative_score
                    .partial_cmp(&a.alternative_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let va_override = va_map.remove(&pr.release_id);

            let low_confidence_reason = match (
                pr.low_confidence_acoustid_ratio,
                pr.low_confidence_avg_album_match,
            ) {
                (Some(acoustid_ratio), Some(avg_album_match)) => {
                    Some(LowConfidenceReason {
                        acoustid_ratio,
                        avg_album_match,
                    })
                }
                _ => None,
            };

            releases.push(ReleaseGroup {
                release_id: pr.release_id,
                release_title: pr.release_title,
                release_artist: pr.release_artist,
                tracks,
                unfilled,
                coverage,
                total_tracks: pr.total_tracks,
                alternatives,
                va_override,
                low_confidence_reason,
            });
        }

        // Sort releases: coverage desc, then total_tracks desc
        releases.sort_by(|a, b| {
            b.coverage
                .partial_cmp(&a.coverage)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.total_tracks.cmp(&a.total_tracks))
        });

        let mut state = Self {
            category,
            entries: Vec::new(),
            cursor: 0,
            scroll: 0,
            track_cursor: 0,
            track_scroll: 0,
            detail_scroll: 0,
            focused_pane: FocusedPane::LeftPane,
            click_targets: Default::default(),
            pin_input: None,
            pin_error: None,
            pinned_release_ids: HashSet::new(),
            releases,
            unmatched: Vec::new(),
        };
        state.rebuild_entries();
        state
    }

    /// Build browser state for unsolved corpus files.
    pub fn build_unmatched(
        category: PackingCategory,
        unmatched_rows: Vec<(i64, String, UnmatchedCorpusTrackData)>,
    ) -> Self {
        let unmatched: Vec<UnmatchedEntry> = unmatched_rows
            .into_iter()
            .map(|(_inode, path, data)| UnmatchedEntry { path, data })
            .collect();

        let mut state = Self {
            category,
            entries: Vec::new(),
            cursor: 0,
            scroll: 0,
            track_cursor: 0,
            track_scroll: 0,
            detail_scroll: 0,
            focused_pane: FocusedPane::LeftPane,
            click_targets: Default::default(),
            pin_input: None,
            pin_error: None,
            pinned_release_ids: HashSet::new(),
            releases: Vec::new(),
            unmatched,
        };
        state.rebuild_entries();
        state
    }

    /// Regenerate the flat entry list from source data.
    pub fn rebuild_entries(&mut self) {
        let mut entries = Vec::new();

        match self.category {
            PackingCategory::Perfect
            | PackingCategory::FullMatches
            | PackingCategory::Singles
            | PackingCategory::Incomplete
            | PackingCategory::LowConfidence => {
                for idx in 0..self.releases.len() {
                    entries.push(PackingListEntry::Release { idx });
                }
            }
            PackingCategory::UnsolvedConflict
            | PackingCategory::UnsolvedNoRelease
            | PackingCategory::UnsolvedNoMatch => {
                for idx in 0..self.unmatched.len() {
                    entries.push(PackingListEntry::Unmatched { idx });
                }
            }
            PackingCategory::Knots => {
                // Knots have their own browser — this category is never used here
            }
        }

        self.entries = entries;
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&PackingListEntry> {
        self.entries.get(self.cursor)
    }

    /// Get the release for the current left-pane selection, if any.
    pub fn selected_release(&self) -> Option<&ReleaseGroup> {
        match self.selected_entry()? {
            PackingListEntry::Release { idx } => self.releases.get(*idx),
            _ => None,
        }
    }

    /// Total track+unfilled count for the selected release (for middle pane bounds).
    pub fn selected_release_item_count(&self) -> usize {
        match self.selected_release() {
            Some(r) => r.tracks.len() + r.unfilled.len(),
            None => 0,
        }
    }
}

// ============================================================================
// Input Handling
// ============================================================================

impl ReleasePackingBrowserState {
    pub fn handle_input(&mut self, action: &InputAction) -> ReleasePackingBrowserAction {
        // When pin input is active, route all input there first
        if self.pin_input.is_some() {
            return self.handle_pin_input(action);
        }

        match self.focused_pane {
            FocusedPane::LeftPane => self.handle_left_input(action),
            FocusedPane::MiddlePane => self.handle_middle_input(action),
            FocusedPane::DetailPane => self.handle_detail_input(action),
        }
    }

    /// Handle input for the pin release UUID field.
    ///
    /// The TextInputState stores raw hex characters only (no dashes, max 32).
    /// Only hex chars are accepted; pasting a full UUID or MB URL is cleaned
    /// automatically. On confirm, the 32 hex chars are formatted as a UUID.
    fn handle_pin_input(&mut self, action: &InputAction) -> ReleasePackingBrowserAction {
        match action {
            InputAction::Confirm => {
                let hex = self.pin_input.as_ref().unwrap().value().to_string();
                if hex.len() != 32 {
                    self.pin_error = Some(format!(
                        "Need 32 hex characters (have {})",
                        hex.len()
                    ));
                    return ReleasePackingBrowserAction::None;
                }
                // Format as UUID: 8-4-4-4-12
                let release_id = format!(
                    "{}-{}-{}-{}-{}",
                    &hex[0..8],
                    &hex[8..12],
                    &hex[12..16],
                    &hex[16..20],
                    &hex[20..32]
                );
                let track_paths = self
                    .selected_release()
                    .map(|r| r.tracks.iter().map(|t| t.path.clone()).collect())
                    .unwrap_or_default();
                self.pin_input = None;
                self.pin_error = None;
                ReleasePackingBrowserAction::PinRelease {
                    release_id,
                    track_paths,
                }
            }
            InputAction::Cancel => {
                self.pin_input = None;
                self.pin_error = None;
                ReleasePackingBrowserAction::None
            }
            InputAction::Char(c) if c.is_ascii_hexdigit() => {
                self.pin_error = None;
                let input = self.pin_input.as_mut().unwrap();
                if input.value().len() < 32 {
                    input.insert_char(c.to_ascii_lowercase());
                }
                ReleasePackingBrowserAction::None
            }
            InputAction::Paste(text) => {
                self.pin_error = None;
                // Strip MB URL prefix, dashes; keep only hex chars
                let trimmed = text.trim();
                let stripped = trimmed
                    .strip_prefix("https://musicbrainz.org/release/")
                    .or_else(|| trimmed.strip_prefix("http://musicbrainz.org/release/"))
                    .unwrap_or(trimmed);
                let hex: String = stripped
                    .chars()
                    .filter(|c| c.is_ascii_hexdigit())
                    .map(|c| c.to_ascii_lowercase())
                    .take(32)
                    .collect();
                let input = self.pin_input.as_mut().unwrap();
                input.clear();
                input.insert_str(&hex);
                ReleasePackingBrowserAction::None
            }
            InputAction::Backspace
            | InputAction::Delete
            | InputAction::NavLeft
            | InputAction::NavRight
            | InputAction::Home
            | InputAction::End
            | InputAction::TextHome
            | InputAction::TextEnd
            | InputAction::WordLeft
            | InputAction::WordRight
            | InputAction::KillToStart
            | InputAction::KillToEnd => {
                self.pin_error = None;
                self.pin_input.as_mut().unwrap().handle_input(action);
                ReleasePackingBrowserAction::None
            }
            _ => ReleasePackingBrowserAction::None,
        }
    }

    /// Whether a release is selected (i.e., pinning is possible).
    fn has_selected_release(&self) -> bool {
        self.selected_release().is_some()
    }

    fn handle_left_input(&mut self, action: &InputAction) -> ReleasePackingBrowserAction {
        match action {
            InputAction::NavUp => {
                self.move_cursor_up();
                self.reset_middle_pane();
                ReleasePackingBrowserAction::None
            }
            InputAction::NavDown => {
                self.move_cursor_down();
                self.reset_middle_pane();
                ReleasePackingBrowserAction::None
            }
            InputAction::NavRight | InputAction::FocusRight => {
                self.focused_pane = FocusedPane::MiddlePane;
                ReleasePackingBrowserAction::None
            }
            InputAction::Home => {
                self.cursor = 0;
                self.reset_middle_pane();
                ReleasePackingBrowserAction::None
            }
            InputAction::End => {
                if !self.entries.is_empty() {
                    self.cursor = self.entries.len() - 1;
                }
                self.reset_middle_pane();
                ReleasePackingBrowserAction::None
            }
            InputAction::Char('p') if self.has_selected_release() => {
                self.pin_input = Some(TextInputState::new());
                self.pin_error = None;
                ReleasePackingBrowserAction::None
            }
            InputAction::Cancel => ReleasePackingBrowserAction::Cancel,
            _ => ReleasePackingBrowserAction::None,
        }
    }

    fn handle_middle_input(&mut self, action: &InputAction) -> ReleasePackingBrowserAction {
        let item_count = self.selected_release_item_count();

        match action {
            InputAction::NavUp => {
                if self.track_cursor > 0 {
                    self.track_cursor -= 1;
                    self.detail_scroll = 0;
                }
                ReleasePackingBrowserAction::None
            }
            InputAction::NavDown => {
                if item_count > 0 && self.track_cursor < item_count - 1 {
                    self.track_cursor += 1;
                    self.detail_scroll = 0;
                }
                ReleasePackingBrowserAction::None
            }
            InputAction::NavLeft | InputAction::FocusLeft => {
                self.focused_pane = FocusedPane::LeftPane;
                ReleasePackingBrowserAction::None
            }
            InputAction::FocusRight => {
                self.focused_pane = FocusedPane::DetailPane;
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::Home => {
                self.track_cursor = 0;
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::End => {
                if item_count > 0 {
                    self.track_cursor = item_count - 1;
                }
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::Char('p') if self.has_selected_release() => {
                self.pin_input = Some(TextInputState::new());
                self.pin_error = None;
                ReleasePackingBrowserAction::None
            }
            InputAction::Cancel => ReleasePackingBrowserAction::Cancel,
            _ => ReleasePackingBrowserAction::None,
        }
    }

    fn handle_detail_input(&mut self, action: &InputAction) -> ReleasePackingBrowserAction {
        match action {
            InputAction::NavUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(1);
                ReleasePackingBrowserAction::None
            }
            InputAction::NavDown => {
                self.detail_scroll += 1;
                ReleasePackingBrowserAction::None
            }
            InputAction::PageUp => {
                self.detail_scroll = self.detail_scroll.saturating_sub(20);
                ReleasePackingBrowserAction::None
            }
            InputAction::PageDown => {
                self.detail_scroll += 20;
                ReleasePackingBrowserAction::None
            }
            InputAction::Home => {
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::End => {
                self.detail_scroll = usize::MAX / 2;
                ReleasePackingBrowserAction::None
            }
            InputAction::NavLeft | InputAction::FocusLeft => {
                self.focused_pane = FocusedPane::MiddlePane;
                ReleasePackingBrowserAction::None
            }
            InputAction::Cancel => ReleasePackingBrowserAction::Cancel,
            _ => ReleasePackingBrowserAction::None,
        }
    }

    fn reset_middle_pane(&mut self) {
        self.track_cursor = 0;
        self.track_scroll = 0;
        self.detail_scroll = 0;
    }

    fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    fn move_cursor_down(&mut self) {
        if self.cursor + 1 < self.entries.len() {
            self.cursor += 1;
        }
    }
}
