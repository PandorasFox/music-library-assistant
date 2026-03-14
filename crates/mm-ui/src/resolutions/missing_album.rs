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

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::signals::data::{MissingAlbumSingleData, SingleTrackInfo};

    fn make_track(index: usize) -> SingleTrackInfo {
        SingleTrackInfo {
            inode: index as i64,
            title: format!("Track {index}"),
            path: format!("corpus/artist/track_{index}.flac"),
        }
    }

    fn make_signal(artist: &str, num_tracks: usize) -> MissingAlbumSingleSignalWire {
        let tracks = (0..num_tracks).map(make_track).collect();
        MissingAlbumSingleSignalWire {
            key: format!("missing_album:{}", artist.to_lowercase()),
            data: MissingAlbumSingleData {
                artist: artist.to_string(),
                tracks,
            },
        }
    }

    fn make_signals(num_groups: usize, tracks_per: usize) -> Vec<MissingAlbumSingleSignalWire> {
        (0..num_groups)
            .map(|i| make_signal(&format!("Artist {i}"), tracks_per))
            .collect()
    }

    // -- GroupNavigation tests --

    #[test]
    fn zero_groups_navigation() {
        let data = MissingAlbumData::new(vec![]);
        assert_eq!(data.group_count(), 0);
        assert!(!data.has_next());
        assert!(!data.has_prev());
    }

    #[test]
    fn three_groups_navigation() {
        let mut data = MissingAlbumData::new(make_signals(3, 2));
        assert_eq!(data.group_count(), 3);
        assert!(data.has_next());
        assert!(!data.has_prev());

        data.current_group = 1;
        assert!(data.has_next());
        assert!(data.has_prev());

        data.current_group = 2;
        assert!(!data.has_next());
        assert!(data.has_prev());
    }

    // -- ResolutionData tests --

    #[test]
    fn list_len_returns_track_count() {
        let data = MissingAlbumData::new(vec![make_signal("Foo", 4)]);
        assert_eq!(data.list_len(), 4);
    }

    #[test]
    fn list_len_zero_when_empty() {
        let data = MissingAlbumData::new(vec![]);
        assert_eq!(data.list_len(), 0);
    }

    #[test]
    fn list_len_changes_with_current_group() {
        let signals = vec![make_signal("A", 2), make_signal("B", 5)];
        let mut data = MissingAlbumData::new(signals);
        assert_eq!(data.list_len(), 2);
        data.current_group = 1;
        assert_eq!(data.list_len(), 5);
    }

    #[test]
    fn selected_path_valid_cursor() {
        let data = MissingAlbumData::new(vec![make_signal("X", 3)]);
        assert_eq!(data.selected_path(0), Some("corpus/artist/track_0.flac"));
        assert_eq!(data.selected_path(2), Some("corpus/artist/track_2.flac"));
    }

    #[test]
    fn selected_path_out_of_bounds() {
        let data = MissingAlbumData::new(vec![make_signal("X", 1)]);
        assert!(data.selected_path(99).is_none());
    }

    #[test]
    fn list_title_includes_artist_and_position() {
        let data = MissingAlbumData::new(make_signals(3, 2));
        let title = data.list_title();
        assert!(
            title.contains("Artist 0"),
            "expected artist name in: {title}"
        );
        assert!(title.contains("1/3"), "expected '1/3' in: {title}");
        assert!(
            title.contains("2 tracks"),
            "expected '2 tracks' in: {title}"
        );
    }

    // -- ModalButtons tests --

    #[test]
    fn action_buttons_enabled_when_has_tracks() {
        let ctx = MissingAlbumButtonCtx {
            has_tracks: true,
            group_index: 0,
        };
        assert!(MissingAlbumButton::PerTrackTitle.enabled(&ctx));
        assert!(MissingAlbumButton::AllSingles.enabled(&ctx));
        assert!(MissingAlbumButton::Suppress.enabled(&ctx));
    }

    #[test]
    fn action_buttons_disabled_when_empty() {
        let ctx = MissingAlbumButtonCtx {
            has_tracks: false,
            group_index: 0,
        };
        assert!(!MissingAlbumButton::PerTrackTitle.enabled(&ctx));
        assert!(!MissingAlbumButton::AllSingles.enabled(&ctx));
        assert!(!MissingAlbumButton::Suppress.enabled(&ctx));
    }

    #[test]
    fn cancel_always_enabled() {
        for has_tracks in [true, false] {
            let ctx = MissingAlbumButtonCtx {
                has_tracks,
                group_index: 0,
            };
            assert!(MissingAlbumButton::Cancel.enabled(&ctx));
        }
    }

    #[test]
    fn actions_return_correct_variants() {
        let ctx = MissingAlbumButtonCtx {
            has_tracks: true,
            group_index: 0,
        };
        assert_eq!(
            MissingAlbumButton::PerTrackTitle.action(&ctx),
            MissingAlbumAction::PerTrackTitle
        );
        assert_eq!(
            MissingAlbumButton::AllSingles.action(&ctx),
            MissingAlbumAction::AllSingles
        );
        assert_eq!(
            MissingAlbumButton::Suppress.action(&ctx),
            MissingAlbumAction::Suppress
        );
        assert_eq!(
            MissingAlbumButton::Cancel.action(&ctx),
            MissingAlbumAction::Cancel
        );
    }
}
