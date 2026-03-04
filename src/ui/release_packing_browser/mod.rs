//! Release Packing Browser — read-only full-screen browser for packing results.
//!
//! Three-pane layout:
//! - Left: flat list of releases, near-misses, and unmatched files
//! - Middle: tracks/slots for the selected release
//! - Bottom-right: per-track detail (score breakdown, file info)

pub mod render;
pub mod types;

use std::collections::HashMap;

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
    // Left pane (flat list: releases + near-miss + unmatched)
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

    // Source data
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
            track_cursor: 0,
            track_scroll: 0,
            detail_scroll: 0,
            focused_pane: FocusedPane::LeftPane,
            click_targets: Default::default(),
            releases,
            near_misses: near_miss_rows,
            unmatched,
            total_assigned,
            total_releases,
        };
        state.rebuild_entries();
        state.advance_cursor_to_navigable(0);
        state
    }

    /// Regenerate the flat entry list from source data.
    pub fn rebuild_entries(&mut self) {
        let mut entries = Vec::new();

        if !self.releases.is_empty() {
            entries.push(PackingListEntry::ReleaseSectionHeader {
                count: self.releases.len(),
            });
            for idx in 0..self.releases.len() {
                entries.push(PackingListEntry::ReleaseHeader { release_idx: idx });
            }
        }

        if !self.near_misses.is_empty() {
            entries.push(PackingListEntry::NearMissSectionHeader {
                count: self.near_misses.len(),
            });
            for idx in 0..self.near_misses.len() {
                entries.push(PackingListEntry::NearMissEntry { idx });
            }
        }

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
        self.cursor = from.min(self.entries.len().saturating_sub(1));
    }

    /// Get the currently selected entry.
    pub fn selected_entry(&self) -> Option<&PackingListEntry> {
        self.entries.get(self.cursor)
    }

    /// Get the release for the current left-pane selection, if any.
    pub fn selected_release(&self) -> Option<&ReleaseGroup> {
        match self.selected_entry()? {
            PackingListEntry::ReleaseHeader { release_idx } => {
                self.releases.get(*release_idx)
            }
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
        match self.focused_pane {
            FocusedPane::LeftPane => self.handle_left_input(action),
            FocusedPane::MiddlePane => self.handle_middle_input(action),
            FocusedPane::DetailPane => self.handle_detail_input(action),
        }
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
                self.advance_cursor_to_navigable(0);
                self.reset_middle_pane();
                ReleasePackingBrowserAction::None
            }
            InputAction::End => {
                for i in (0..self.entries.len()).rev() {
                    if !self.entries[i].is_section_header() {
                        self.cursor = i;
                        break;
                    }
                }
                self.reset_middle_pane();
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
