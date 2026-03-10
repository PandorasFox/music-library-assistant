//! Knot browser — read-only view for inspecting packing knot components.
//!
//! Tab/Shift+Tab cycles between knots. Arrow keys navigate proposals.
//! Z opens wizard pane with full proposal detail.

pub mod render;
pub mod types;

use std::collections::{BTreeSet, HashMap, HashSet};


use crate::meta::signals::data::PackingKnotData;
use crate::ui::input::InputAction;
use crate::ui::widgets::standard_list::{
    ListEntry, ListInputResult, StandardListConfig, StandardListState,
};
use crate::ui::widgets::wizard::{WizardItem, WizardOffer};

use types::{KnotEntry, KnotProposal, KnotProposalTrack, KnotReleaseSortMode};

// ============================================================================
// Actions
// ============================================================================

pub(crate) enum KnotBrowserAction {
    None,
    Cancel,
}

// ============================================================================
// WizardItem + ListEntry impls for KnotProposal
// ============================================================================

impl WizardItem for KnotProposal {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let content = render::build_proposal_detail_blocks(self);
        if content.is_empty() {
            None
        } else {
            Some(WizardOffer::Pane {
                title: self.release_title.clone(),
                content,
            })
        }
    }
}

impl ListEntry for KnotProposal {
    type Action = ();

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None // Read-only browser
    }
}

// ============================================================================
// State
// ============================================================================

pub(crate) struct KnotBrowserState {
    pub knots: Vec<KnotEntry>,
    /// Current knot index (Tab/Shift+Tab cycles this).
    pub knot_index: usize,
    pub sort_mode: KnotReleaseSortMode,
    pub list: StandardListState,
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
                }
            })
            .collect();

        // Sort knots by total graph vertices (proposals + inodes) descending
        knots.sort_by(|a, b| {
            let va = a.proposal_count + a.inode_count;
            let vb = b.proposal_count + b.inode_count;
            vb.cmp(&va)
        });

        let sort_mode = KnotReleaseSortMode::ByInodeCount;
        let mut state = Self {
            knots,
            knot_index: 0,
            sort_mode,
            list: StandardListState::new(StandardListConfig::default()),
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

}

// ============================================================================
// Input handling
// ============================================================================

impl KnotBrowserState {
    pub fn handle_input(&mut self, action: &InputAction) -> KnotBrowserAction {
        let proposals = self
            .knots
            .get(self.knot_index)
            .map(|k| k.proposals.as_slice())
            .unwrap_or(&[]);

        match self.list.handle_input(action, proposals) {
            ListInputResult::Consumed | ListInputResult::CursorMoved | ListInputResult::Toggled => {
                return KnotBrowserAction::None;
            }
            ListInputResult::Confirm(()) => return KnotBrowserAction::None,
            ListInputResult::Unhandled => {}
        }

        // Handle actions not consumed by StandardList
        match action {
            InputAction::Cancel => KnotBrowserAction::Cancel,

            // Tab/Shift+Tab: cycle knots
            InputAction::CycleNext => {
                if !self.knots.is_empty() && self.knot_index + 1 < self.knots.len() {
                    self.knot_index += 1;
                    self.list.reset();
                    self.sort_current_knot();
                }
                KnotBrowserAction::None
            }
            InputAction::CyclePrev => {
                if self.knot_index > 0 {
                    self.knot_index -= 1;
                    self.list.reset();
                    self.sort_current_knot();
                }
                KnotBrowserAction::None
            }

            // Sort toggle
            InputAction::Char('s') => {
                self.sort_mode = self.sort_mode.toggle();
                self.sort_current_knot();
                if let Some(knot) = self.knots.get(self.knot_index) {
                    self.list.clamp_cursor(&knot.proposals);
                }
                KnotBrowserAction::None
            }

            _ => KnotBrowserAction::None,
        }
    }

    /// Handle mouse click for cursor selection.
    pub fn handle_click(&mut self, x: u16, y: u16) {
        if let Some(knot) = self.knots.get(self.knot_index) {
            self.list.handle_click(x, y, &knot.proposals);
        }
    }
}

// ============================================================================
// Construction helpers
// ============================================================================

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
