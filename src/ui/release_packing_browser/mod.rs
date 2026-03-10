//! Release Packing Browser — full-screen browser for packing results.
//!
//! Single-pane StandardList with wizard integration:
//! - List: release/unmatched entries with multi-select for bulk approval
//! - Wizard popup (z): release overview
//! - Wizard pane (Z): interleaved tracks + score breakdown cards

pub mod render;
pub mod types;

use std::collections::{BTreeSet, HashMap, HashSet};

use crate::meta::signals::data::{
    AlternativeReleasePackingData, PackedReleaseData, ReleasePackingData, UnfilledReleaseSlotData,
    UnmatchedCorpusTrackData, VariousArtistsOverrideData, VariousArtistsOverrideSource,
};
use crate::ui::input::InputAction;
use crate::ui::widgets::standard_list::{ListInputResult, StandardListConfig, StandardListState};
use crate::ui::widgets::TextInputState;
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
    ApproveSelected {
        selected_indices: BTreeSet<usize>,
    },
}

// ============================================================================
// State
// ============================================================================

pub(crate) struct ReleasePackingBrowserState {
    /// Which category this browser is showing.
    pub category: PackingCategory,

    /// Flat list of entries (releases or unmatched files).
    pub entries: Vec<PackingListEntry>,

    /// StandardList state: cursor, scroll, selection, wizard.
    pub list_state: StandardListState,

    /// Pin release input overlay.
    pub pin_input: Option<TextInputState>,
    pub pin_error: Option<String>,

    /// Release IDs pinned during this browser session (for visual feedback).
    pub pinned_release_ids: HashSet<String>,

    /// Source data (only the category being viewed is populated).
    pub releases: Vec<ReleaseGroup>,
    pub unmatched: Vec<UnmatchedEntry>,
}

// ============================================================================
// Construction
// ============================================================================

impl ReleasePackingBrowserState {
    /// Build browser state for a release category (FullMatches, Singles, Incomplete, etc.)
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

        // Build release groups from PackedReleaseData
        let mut releases: Vec<ReleaseGroup> = Vec::new();

        for pr in packed {
            let mut tracks: Vec<AssignedTrackInfo> = track_map
                .remove(&pr.release_id)
                .unwrap_or_default()
                .into_iter()
                .map(|(inode, path, data)| AssignedTrackInfo {
                    inode,
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
            list_state: StandardListState::new(StandardListConfig { multi_select: true }),
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
            list_state: StandardListState::new(StandardListConfig { multi_select: true }),
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
                    let release = &self.releases[idx];
                    let popup_lines =
                        render::build_release_overview_lines(release, self.category);
                    let pane_title = format!("Tracks: {}", release.release_title);
                    let pane_content =
                        render::build_track_pane_content(release, self.category);
                    entries.push(PackingListEntry::Release {
                        idx,
                        popup_lines,
                        pane_title,
                        pane_content,
                    });
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

        // Pre-select based on category
        self.list_state.selected.clear();
        match self.category {
            PackingCategory::Perfect
            | PackingCategory::FullMatches
            | PackingCategory::Singles
            | PackingCategory::Incomplete => {
                // All release entries selected by default
                for (i, entry) in self.entries.iter().enumerate() {
                    if matches!(entry, PackingListEntry::Release { .. }) {
                        self.list_state.selected.insert(i);
                    }
                }
            }
            PackingCategory::LowConfidence => {
                // None selected by default — operator opts in
            }
            _ => {}
        }
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&PackingListEntry> {
        self.entries.get(self.list_state.cursor)
    }

    /// Get the release for the current selection, if any.
    pub fn selected_release(&self) -> Option<&ReleaseGroup> {
        match self.selected_entry()? {
            PackingListEntry::Release { idx, .. } => self.releases.get(*idx),
            _ => None,
        }
    }

    /// Collect selected release data for approval.
    ///
    /// Maps selected list indices → release groups, extracting the per-track
    /// data needed for tag generation. Skips non-release entries.
    pub fn collect_selected_releases(
        &self,
        selected_indices: &BTreeSet<usize>,
    ) -> Vec<SelectedReleaseData> {
        selected_indices
            .iter()
            .filter_map(|&idx| {
                let entry = self.entries.get(idx)?;
                match entry {
                    PackingListEntry::Release { idx: rel_idx, .. } => {
                        let release = self.releases.get(*rel_idx)?;
                        Some(SelectedReleaseData {
                            release_id: release.release_id.clone(),
                            tracks: release
                                .tracks
                                .iter()
                                .map(|t| SelectedTrackData {
                                    inode: t.inode,
                                    track_title: t.track_title.clone(),
                                    track_position: t.track_position,
                                    medium_position: t.medium_position,
                                    recording_id: t.recording_id.clone(),
                                })
                                .collect(),
                        })
                    }
                    _ => None,
                }
            })
            .collect()
    }
}

/// Data extracted from a selected release for approval processing.
pub(crate) struct SelectedReleaseData {
    pub release_id: String,
    pub tracks: Vec<SelectedTrackData>,
}

/// Per-track data needed for tag generation.
pub(crate) struct SelectedTrackData {
    pub inode: i64,
    pub track_title: String,
    pub track_position: u32,
    pub medium_position: u32,
    pub recording_id: String,
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

        // Pin release shortcut
        if matches!(action, InputAction::Char('p')) && self.selected_release().is_some() {
            self.pin_input = Some(TextInputState::new());
            self.pin_error = None;
            return ReleasePackingBrowserAction::None;
        }

        // Cancel
        if matches!(action, InputAction::Cancel) {
            return ReleasePackingBrowserAction::Cancel;
        }

        // Delegate to StandardList
        match self.list_state.handle_input(action, &self.entries) {
            ListInputResult::Confirm(action) => action,
            ListInputResult::Consumed
            | ListInputResult::CursorMoved
            | ListInputResult::Toggled => ReleasePackingBrowserAction::None,
            ListInputResult::Unhandled => ReleasePackingBrowserAction::None,
        }
    }

    /// Handle input for the pin release UUID field.
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
}
