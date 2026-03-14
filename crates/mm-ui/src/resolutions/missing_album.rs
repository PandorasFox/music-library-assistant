//! Missing Album Singles resolution — tracks without ALBUM tag, grouped by artist.
//!
//! Route: `/resolve/missing-album-singles`
//! Query: `GetMissingAlbumSingleSignals`
//! Data: `Vec<MissingAlbumSingleSignalWire>` (mm-meta)
//! Mutations: tag writes per group (PerTrackTitle, AllSingles) or suppress

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::domain_queries::MissingAlbumSingleSignalWire;

use crate::group_navigation::GroupNavigation;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the signal wire list to implement `ResolutionData` + `GroupNavigation`.
pub struct MissingAlbumData {
    pub signals: Vec<MissingAlbumSingleSignalWire>,
    pub current_group: usize,
}

impl MissingAlbumData {
    pub fn new(signals: Vec<MissingAlbumSingleSignalWire>) -> Self {
        Self {
            signals,
            current_group: 0,
        }
    }
}

impl ResolutionData for MissingAlbumData {
    type ButtonCtx = MissingAlbumButtonCtx;

    fn list_len(&self) -> usize {
        self.signals
            .get(self.current_group)
            .map_or(0, |s| s.data.tracks.len())
    }

    fn button_ctx(&self) -> MissingAlbumButtonCtx {
        MissingAlbumButtonCtx {
            has_tracks: self.list_len() > 0,
            group_index: self.current_group,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.signals
            .get(self.current_group)
            .and_then(|s| s.data.tracks.get(cursor))
            .map(|t| t.path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 3,
        }
    }

    fn list_title(&self) -> String {
        let track_count = self.list_len();
        let group_num = self.current_group + 1;
        let total = self.signals.len();
        let artist = self
            .signals
            .get(self.current_group)
            .map(|s| s.data.artist.as_str())
            .unwrap_or("?");
        format!(
            " {} ({} tracks) [group {}/{}] ",
            artist, track_count, group_num, total
        )
    }

    fn empty_message(&self) -> &'static str {
        "No missing album singles"
    }
}

impl GroupNavigation for MissingAlbumData {
    fn group_count(&self) -> usize {
        self.signals.len()
    }

    fn current_group(&self) -> usize {
        self.current_group
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for missing album modals.
pub type MissingAlbumState = ResolutionState<MissingAlbumData, MissingAlbumButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingAlbumAction {
    PerTrackTitle,
    AllSingles,
    Suppress,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingAlbumButton {
    PerTrackTitle,
    AllSingles,
    Suppress,
    #[default]
    Cancel,
}

pub struct MissingAlbumButtonCtx {
    pub has_tracks: bool,
    pub group_index: usize,
}

impl ModalButtons for MissingAlbumButton {
    type Context = MissingAlbumButtonCtx;
    type Action = MissingAlbumAction;

    fn all() -> &'static [Self] {
        &[
            Self::PerTrackTitle,
            Self::AllSingles,
            Self::Suppress,
            Self::Cancel,
        ]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::PerTrackTitle => "Per-Track Title".into(),
            Self::AllSingles => "All Singles".into(),
            Self::Suppress => "Suppress".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::PerTrackTitle if ctx.has_tracks => Color::Green,
            Self::PerTrackTitle => Color::DarkGray,
            Self::AllSingles if ctx.has_tracks => Color::Cyan,
            Self::AllSingles => Color::DarkGray,
            Self::Suppress if ctx.has_tracks => Color::Yellow,
            Self::Suppress => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::PerTrackTitle | Self::AllSingles | Self::Suppress => ctx.has_tracks,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingAlbumAction {
        match self {
            Self::PerTrackTitle => MissingAlbumAction::PerTrackTitle,
            Self::AllSingles => MissingAlbumAction::AllSingles,
            Self::Suppress => MissingAlbumAction::Suppress,
            Self::Cancel => MissingAlbumAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::PerTrackTitle | Self::AllSingles | Self::Suppress => {
                ProtocolBinding::Transaction {
                    decision_key: DecisionKey::MissingAlbum {
                        group_index: ctx.group_index,
                    },
                    label: match self {
                        Self::PerTrackTitle => "Set ALBUM = per-track title".into(),
                        Self::AllSingles => "Set ALBUM = \"Singles\"".into(),
                        Self::Suppress => "Suppress missing album signal".into(),
                        _ => unreachable!(),
                    },
                }
            }
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
