//! Knot browser — read-only two-pane view for inspecting packing knot components.
//!
//! Tab/Shift+Tab cycles between knots. Arrow keys navigate the releases pane
//! or scroll the detail pane. Shift+Arrow switches pane focus.

pub mod render;
pub mod types;

use std::collections::{HashMap, HashSet};

use crate::meta::signals::data::PackingKnotData;
use crate::ui::input::InputAction;
use crate::ui::widgets::ListClickTargets;

use types::{
    ContestedInode, KnotEntry, KnotProposal, KnotProposalTrack, KnotReleaseSortMode,
};

// ============================================================================
// Actions
// ============================================================================

pub(crate) enum KnotBrowserAction {
    None,
    Cancel,
}

// ============================================================================
// Pane focus
// ============================================================================

pub(crate) enum FocusedPane {
    /// Left pane: release list.
    Releases,
    /// Right pane: detail (scrollable).
    Detail,
}

// ============================================================================
// State
// ============================================================================

pub(crate) struct KnotBrowserState {
    pub knots: Vec<KnotEntry>,
    /// Current knot index (Tab/Shift+Tab cycles this).
    pub knot_index: usize,
    /// Cursor within the releases pane.
    pub release_cursor: usize,
    pub release_scroll: usize,
    /// Scroll offset for the detail pane.
    pub detail_scroll: usize,
    pub focused_pane: FocusedPane,
    pub sort_mode: KnotReleaseSortMode,
    pub click_targets: ListClickTargets,
}

// ============================================================================
// Construction
// ============================================================================

impl KnotBrowserState {
    pub fn build(
        knot_signals: Vec<PackingKnotData>,
        corpus_paths: &HashMap<i64, String>,
    ) -> Self {
        let mut knots: Vec<KnotEntry> = knot_signals
            .into_iter()
            .map(|data| {
                let contested_inodes = build_contested_inodes(&data, corpus_paths);
                let proposals = build_proposals(&data, corpus_paths);
                KnotEntry {
                    tier: data.tier,
                    proposal_count: proposals.len(),
                    inode_count: data.contested_inodes.len(),
                    ratio: data.ratio,
                    classification: match data.classification {
                        crate::meta::signals::data::KnotClassification::ByRatio => {
                            "by_ratio".to_string()
                        }
                        crate::meta::signals::data::KnotClassification::BySize => {
                            "by_size".to_string()
                        }
                    },
                    proposals,
                    contested_inodes,
                }
            })
            .collect();

        // Sort knots by total graph vertices (proposals + inodes) descending
        knots.sort_by(|a, b| {
            let va = a.proposal_count + a.inode_count;
            let vb = b.proposal_count + b.inode_count;
            vb.cmp(&va)
        });

        let mut state = Self {
            knots,
            knot_index: 0,
            release_cursor: 0,
            release_scroll: 0,
            detail_scroll: 0,
            focused_pane: FocusedPane::Releases,
            sort_mode: KnotReleaseSortMode::ByInodeCount,
            click_targets: Default::default(),
        };
        state.sort_current_knot();
        state
    }

    fn sort_current_knot(&mut self) {
        if let Some(knot) = self.knots.get_mut(self.knot_index) {
            match self.sort_mode {
                KnotReleaseSortMode::ByInodeCount => {
                    knot.proposals.sort_by(|a, b| {
                        b.covered_inode_count
                            .cmp(&a.covered_inode_count)
                            .then_with(|| {
                                b.total_score
                                    .partial_cmp(&a.total_score)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                    });
                }
                KnotReleaseSortMode::ByScore => {
                    knot.proposals.sort_by(|a, b| {
                        b.total_score
                            .partial_cmp(&a.total_score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                            .then_with(|| b.covered_inode_count.cmp(&a.covered_inode_count))
                    });
                }
            }
        }
    }

    pub fn current_knot(&self) -> Option<&KnotEntry> {
        self.knots.get(self.knot_index)
    }

    pub fn selected_proposal(&self) -> Option<&KnotProposal> {
        self.current_knot()
            .and_then(|k| k.proposals.get(self.release_cursor))
    }
}

// ============================================================================
// Input handling
// ============================================================================

impl KnotBrowserState {
    pub fn handle_input(&mut self, action: &InputAction) -> KnotBrowserAction {
        match action {
            InputAction::Cancel => KnotBrowserAction::Cancel,

            // Tab/Shift+Tab: cycle knots
            InputAction::CycleNext => {
                if !self.knots.is_empty() && self.knot_index + 1 < self.knots.len() {
                    self.knot_index += 1;
                    self.release_cursor = 0;
                    self.release_scroll = 0;
                    self.detail_scroll = 0;
                    self.sort_current_knot();
                }
                KnotBrowserAction::None
            }
            InputAction::CyclePrev => {
                if self.knot_index > 0 {
                    self.knot_index -= 1;
                    self.release_cursor = 0;
                    self.release_scroll = 0;
                    self.detail_scroll = 0;
                    self.sort_current_knot();
                }
                KnotBrowserAction::None
            }

            // Arrow keys: navigate within focused pane
            InputAction::NavUp => {
                match self.focused_pane {
                    FocusedPane::Releases => {
                        if self.release_cursor > 0 {
                            self.release_cursor -= 1;
                            self.detail_scroll = 0;
                        }
                    }
                    FocusedPane::Detail => {
                        if self.detail_scroll > 0 {
                            self.detail_scroll -= 1;
                        }
                    }
                }
                KnotBrowserAction::None
            }
            InputAction::NavDown => {
                match self.focused_pane {
                    FocusedPane::Releases => {
                        if let Some(knot) = self.current_knot() {
                            if !knot.proposals.is_empty()
                                && self.release_cursor < knot.proposals.len() - 1
                            {
                                self.release_cursor += 1;
                                self.detail_scroll = 0;
                            }
                        }
                    }
                    FocusedPane::Detail => {
                        self.detail_scroll += 1;
                    }
                }
                KnotBrowserAction::None
            }

            // Shift+Arrow: switch pane focus
            InputAction::FocusRight => {
                self.focused_pane = FocusedPane::Detail;
                KnotBrowserAction::None
            }
            InputAction::FocusLeft => {
                self.focused_pane = FocusedPane::Releases;
                KnotBrowserAction::None
            }

            // Home/End in releases pane
            InputAction::Home => {
                if matches!(self.focused_pane, FocusedPane::Releases) {
                    self.release_cursor = 0;
                    self.detail_scroll = 0;
                }
                KnotBrowserAction::None
            }
            InputAction::End => {
                if matches!(self.focused_pane, FocusedPane::Releases) {
                    if let Some(knot) = self.current_knot() {
                        if !knot.proposals.is_empty() {
                            self.release_cursor = knot.proposals.len() - 1;
                            self.detail_scroll = 0;
                        }
                    }
                }
                KnotBrowserAction::None
            }

            // PageUp/PageDown for detail scroll
            InputAction::PageUp => {
                if matches!(self.focused_pane, FocusedPane::Detail) {
                    self.detail_scroll = self.detail_scroll.saturating_sub(10);
                }
                KnotBrowserAction::None
            }
            InputAction::PageDown => {
                if matches!(self.focused_pane, FocusedPane::Detail) {
                    self.detail_scroll += 10;
                }
                KnotBrowserAction::None
            }

            // Sort toggle (bound to 's' key for now)
            InputAction::Char('s') => {
                self.sort_mode = self.sort_mode.toggle();
                self.release_cursor = 0;
                self.release_scroll = 0;
                self.sort_current_knot();
                KnotBrowserAction::None
            }

            _ => KnotBrowserAction::None,
        }
    }

    /// Handle mouse click for cursor selection.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        if let Some(id) = self.click_targets.hit_test(x, y) {
            if let Ok(idx) = id.parse::<usize>() {
                if let Some(knot) = self.current_knot() {
                    if idx < knot.proposals.len() {
                        self.release_cursor = idx;
                        self.detail_scroll = 0;
                    }
                }
            }
        }
    }
}

// ============================================================================
// Construction helpers
// ============================================================================

fn build_contested_inodes(
    data: &PackingKnotData,
    corpus_paths: &HashMap<i64, String>,
) -> Vec<ContestedInode> {
    // Count how many proposals claim each inode
    let mut inode_claims: HashMap<i64, usize> = HashMap::new();
    for proposal in &data.proposals {
        let proposal_inodes: HashSet<i64> =
            proposal.assignments.iter().map(|a| a.inode).collect();
        for inode in proposal_inodes {
            *inode_claims.entry(inode).or_default() += 1;
        }
    }

    let mut contested: Vec<ContestedInode> = data
        .contested_inodes
        .iter()
        .map(|&inode| ContestedInode {
            path: corpus_paths
                .get(&inode)
                .cloned()
                .unwrap_or_else(|| format!("<inode {}>", inode)),
            claiming_release_count: inode_claims.get(&inode).copied().unwrap_or(0),
        })
        .collect();

    // Sort by claim count descending, then path
    contested.sort_by(|a, b| {
        b.claiming_release_count
            .cmp(&a.claiming_release_count)
            .then_with(|| a.path.cmp(&b.path))
    });
    contested
}

fn build_proposals(
    data: &PackingKnotData,
    corpus_paths: &HashMap<i64, String>,
) -> Vec<KnotProposal> {
    data.proposals
        .iter()
        .map(|entry| {
            let knot_inodes: HashSet<i64> = data.contested_inodes.iter().copied().collect();
            let covered_inode_count = entry
                .assignments
                .iter()
                .filter(|a| knot_inodes.contains(&a.inode))
                .map(|a| a.inode)
                .collect::<HashSet<_>>()
                .len();

            let tracks: Vec<KnotProposalTrack> = entry
                .assignments
                .iter()
                .map(|a| KnotProposalTrack {
                    path: corpus_paths
                        .get(&a.inode)
                        .cloned()
                        .unwrap_or_else(|| format!("<inode {}>", a.inode)),
                    medium_pos: a.medium_pos,
                    track_pos: a.track_pos,
                    track_title: a.track_title.clone(),
                    score: a.score,
                    score_breakdown: a.score_breakdown.clone(),
                })
                .collect();

            KnotProposal {
                release_id: entry.release_id.clone(),
                release_title: entry.release_title.clone(),
                release_artist: entry.release_artist.clone(),
                total_tracks: entry.total_tracks,
                total_score: entry.total_score,
                covered_inode_count,
                selected: entry.selected,
                tracks,
            }
        })
        .collect()
}
