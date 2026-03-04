//! Release Packing Browser — read-only full-screen browser for packing results.
//!
//! Shows all release packing signals grouped by release, with unfilled slots,
//! near-misses, and unmatched files in a hierarchical navigable list.

pub mod render;
pub mod types;

use std::collections::{HashMap, HashSet};

use crate::meta::signals::data::{
    NearMissReleaseData, ReleasePackingData, UnfilledReleaseSlotData, UnmatchedCorpusTrackData,
};
use crate::ui::input::InputAction;
use crate::ui::widgets::ListClickTargets;
use types::*;

// ============================================================================
// Actions
// ============================================================================

pub(crate) enum ReleasePackingBrowserAction {
    None,
    Cancel,
}

// ============================================================================
// State
// ============================================================================

pub(crate) struct ReleasePackingBrowserState {
    /// Flat navigable list entries.
    pub entries: Vec<PackingListEntry>,
    /// Cursor position in `entries` (skips section headers).
    pub cursor: usize,
    /// Left pane scroll offset.
    pub scroll: usize,
    /// Right pane scroll offset.
    pub detail_scroll: usize,
    /// Whether the right (detail) pane has focus.
    pub detail_focused: bool,
    /// Click targets for the left pane.
    pub click_targets: ListClickTargets,
    /// Expanded release IDs (all expanded by default).
    pub expanded: HashSet<String>,

    // Source data (for rebuild after expand/collapse)
    pub releases: Vec<ReleaseGroup>,
    pub near_misses: Vec<NearMissReleaseData>,
    pub unmatched: Vec<UnmatchedEntry>,

    // Summary
    pub total_assigned: usize,
    pub total_releases: usize,
}

// ============================================================================
// Construction
// ============================================================================

impl ReleasePackingBrowserState {
    /// Build the browser state from raw signal data.
    pub fn build(
        packing_rows: Vec<(i64, String, ReleasePackingData)>,
        unmatched_rows: Vec<(i64, String, UnmatchedCorpusTrackData)>,
        unfilled_rows: Vec<UnfilledReleaseSlotData>,
        near_miss_rows: Vec<NearMissReleaseData>,
    ) -> Self {
        // Group packing rows by release_id
        let mut release_map: HashMap<String, Vec<(i64, String, ReleasePackingData)>> =
            HashMap::new();
        for row in packing_rows {
            release_map
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

        // Build release groups
        let mut releases: Vec<ReleaseGroup> = Vec::new();
        let mut expanded = HashSet::new();

        for (release_id, rows) in &release_map {
            let first = &rows[0].2;
            let mut tracks: Vec<AssignedTrackInfo> = rows
                .iter()
                .map(|(_inode, path, data)| AssignedTrackInfo {
                    path: path.clone(),
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
                .remove(release_id)
                .unwrap_or_default()
                .into_iter()
                .map(|s| UnfilledSlotInfo {
                    medium_pos: s.medium_pos,
                    track_pos: s.track_pos,
                    track_title: s.track_title,
                    recording_id: s.recording_id,
                })
                .collect();
            unfilled.sort_by(|a, b| a.medium_pos.cmp(&b.medium_pos).then(a.track_pos.cmp(&b.track_pos)));

            let total_tracks = (tracks.len() + unfilled.len()) as u32;
            let coverage = if total_tracks > 0 {
                tracks.len() as f32 / total_tracks as f32
            } else {
                0.0
            };

            expanded.insert(release_id.clone());

            releases.push(ReleaseGroup {
                release_id: release_id.clone(),
                release_title: first.release_title.clone(),
                release_artist: first.release_artist.clone(),
                tracks,
                unfilled,
                coverage,
                total_tracks,
            });
        }

        // Sort releases: coverage desc, then total_tracks desc
        releases.sort_by(|a, b| {
            b.coverage
                .partial_cmp(&a.coverage)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(b.total_tracks.cmp(&a.total_tracks))
        });

        let total_assigned = releases.iter().map(|r| r.tracks.len()).sum();
        let total_releases = releases.len();

        let unmatched: Vec<UnmatchedEntry> = unmatched_rows
            .into_iter()
            .map(|(_inode, path, data)| UnmatchedEntry { path, data })
            .collect();

        let mut state = Self {
            entries: Vec::new(),
            cursor: 0,
            scroll: 0,
            detail_scroll: 0,
            detail_focused: false,
            click_targets: Default::default(),
            expanded,
            releases,
            near_misses: near_miss_rows,
            unmatched,
            total_assigned,
            total_releases,
        };
        state.rebuild_entries();
        // Set cursor to first navigable entry
        state.advance_cursor_to_navigable(0);
        state
    }

    /// Regenerate the flat entry list from source data + expanded set.
    pub fn rebuild_entries(&mut self) {
        let mut entries = Vec::new();

        // Releases section
        if !self.releases.is_empty() {
            entries.push(PackingListEntry::ReleaseSectionHeader {
                count: self.releases.len(),
            });
            for (idx, release) in self.releases.iter().enumerate() {
                let is_expanded = self.expanded.contains(&release.release_id);
                entries.push(PackingListEntry::ReleaseHeader {
                    release_idx: idx,
                    expanded: is_expanded,
                });
                if is_expanded {
                    for track_idx in 0..release.tracks.len() {
                        entries.push(PackingListEntry::AssignedTrack {
                            release_idx: idx,
                            track_idx,
                        });
                    }
                    for slot_idx in 0..release.unfilled.len() {
                        entries.push(PackingListEntry::UnfilledSlot {
                            release_idx: idx,
                            slot_idx,
                        });
                    }
                }
            }
        }

        // Near-misses section
        if !self.near_misses.is_empty() {
            entries.push(PackingListEntry::NearMissSectionHeader {
                count: self.near_misses.len(),
            });
            for idx in 0..self.near_misses.len() {
                entries.push(PackingListEntry::NearMissEntry { idx });
            }
        }

        // Unmatched section
        if !self.unmatched.is_empty() {
            entries.push(PackingListEntry::UnmatchedSectionHeader {
                count: self.unmatched.len(),
            });
            for idx in 0..self.unmatched.len() {
                entries.push(PackingListEntry::UnmatchedFile { idx });
            }
        }

        self.entries = entries;
    }

    /// Advance cursor to the next navigable (non-header) entry at or after `from`.
    fn advance_cursor_to_navigable(&mut self, from: usize) {
        for i in from..self.entries.len() {
            if !self.entries[i].is_section_header() {
                self.cursor = i;
                return;
            }
        }
        // Fallback: stay at from
        self.cursor = from.min(self.entries.len().saturating_sub(1));
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&PackingListEntry> {
        self.entries.get(self.cursor)
    }
}

// ============================================================================
// Input Handling
// ============================================================================

impl ReleasePackingBrowserState {
    pub fn handle_input(&mut self, action: &InputAction) -> ReleasePackingBrowserAction {
        if self.detail_focused {
            return self.handle_detail_input(action);
        }

        match action {
            InputAction::NavUp => {
                self.move_cursor_up();
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::NavDown => {
                self.move_cursor_down();
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::NavRight => {
                // Expand release, or focus detail pane
                if let Some(PackingListEntry::ReleaseHeader { release_idx, expanded: false }) =
                    self.entries.get(self.cursor)
                {
                    let release_id = self.releases[*release_idx].release_id.clone();
                    self.expanded.insert(release_id);
                    self.rebuild_entries();
                } else {
                    self.detail_focused = true;
                    self.detail_scroll = 0;
                }
                ReleasePackingBrowserAction::None
            }
            InputAction::NavLeft => {
                // Collapse release
                if let Some(PackingListEntry::ReleaseHeader { release_idx, expanded: true }) =
                    self.entries.get(self.cursor)
                {
                    let release_id = self.releases[*release_idx].release_id.clone();
                    self.expanded.remove(&release_id);
                    self.rebuild_entries();
                }
                ReleasePackingBrowserAction::None
            }
            InputAction::FocusRight => {
                self.detail_focused = true;
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::Home => {
                self.advance_cursor_to_navigable(0);
                self.detail_scroll = 0;
                ReleasePackingBrowserAction::None
            }
            InputAction::End => {
                // Find last navigable entry
                for i in (0..self.entries.len()).rev() {
                    if !self.entries[i].is_section_header() {
                        self.cursor = i;
                        break;
                    }
                }
                self.detail_scroll = 0;
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
                self.detail_scroll = usize::MAX / 2; // will be clamped in render
                ReleasePackingBrowserAction::None
            }
            InputAction::NavLeft | InputAction::FocusLeft => {
                self.detail_focused = false;
                ReleasePackingBrowserAction::None
            }
            InputAction::Cancel => ReleasePackingBrowserAction::Cancel,
            _ => ReleasePackingBrowserAction::None,
        }
    }

    fn move_cursor_up(&mut self) {
        if self.cursor == 0 {
            return;
        }
        for i in (0..self.cursor).rev() {
            if !self.entries[i].is_section_header() {
                self.cursor = i;
                return;
            }
        }
    }

    fn move_cursor_down(&mut self) {
        for i in (self.cursor + 1)..self.entries.len() {
            if !self.entries[i].is_section_header() {
                self.cursor = i;
                return;
            }
        }
    }
}
