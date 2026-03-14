//! Inconsistent Album Artist resolution — normalize album_artist across a cluster.
//!
//! Route: `/resolve/inconsistent-album-artist`
//! Query: `GetInconsistentAlbumArtistResolutionData`
//! Data: `TagCanonicityResolutionData` (mm-meta, reuses same packed shape)
//! Mutations: canonical tag emit per cluster (Confirm) or flag as non-compilation

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::canonicity_compound::TagCanonicityResolutionData;

use crate::group_navigation::GroupNavigation;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta packed type to implement `ResolutionData` + `GroupNavigation`.
///
/// Same data shape as tag canonicity (reuses `TagCanonicityResolutionData`),
/// but different buttons and semantics for album artist inconsistency.
pub struct AlbumArtistData {
    pub inner: TagCanonicityResolutionData,
    pub current_cluster: usize,
}

impl AlbumArtistData {
    pub fn new(data: TagCanonicityResolutionData) -> Self {
        Self {
            inner: data,
            current_cluster: 0,
        }
    }
}

impl ResolutionData for AlbumArtistData {
    type ButtonCtx = AlbumArtistButtonCtx;

    fn list_len(&self) -> usize {
        self.inner
            .clusters
            .get(self.current_cluster)
            .map_or(0, |c| c.outlier_variants.len())
    }

    fn button_ctx(&self) -> AlbumArtistButtonCtx {
        AlbumArtistButtonCtx {
            has_outliers: self.list_len() > 0,
            current_cluster_index: self.current_cluster,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.inner
            .clusters
            .get(self.current_cluster)
            .and_then(|c| c.outlier_variants.get(cursor))
            .and_then(|v| v.files.first())
            .map(|f| f.display_name.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FieldAboveList {
            field_height: 3,
            detail_height: 3,
        }
    }

    fn list_title(&self) -> String {
        let current = self.current_cluster + 1;
        let total = self.inner.clusters.len();
        format!(" {} ({}/{}) ", self.inner.tag_name, current, total)
    }

    fn empty_message(&self) -> &'static str {
        "No outlier variants"
    }
}

impl GroupNavigation for AlbumArtistData {
    fn group_count(&self) -> usize {
        self.inner.clusters.len()
    }

    fn current_group(&self) -> usize {
        self.current_cluster
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for inconsistent album artist modals.
pub type AlbumArtistState = ResolutionState<AlbumArtistData, AlbumArtistButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlbumArtistAction {
    Confirm,
    FlagNonCompilation,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AlbumArtistButton {
    Confirm,
    FlagNonCompilation,
    #[default]
    Cancel,
}

pub struct AlbumArtistButtonCtx {
    pub has_outliers: bool,
    pub current_cluster_index: usize,
}

impl ModalButtons for AlbumArtistButton {
    type Context = AlbumArtistButtonCtx;
    type Action = AlbumArtistAction;

    fn all() -> &'static [Self] {
        &[Self::Confirm, Self::FlagNonCompilation, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Confirm => "Confirm".into(),
            Self::FlagNonCompilation => "Not a Compilation".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Confirm if ctx.has_outliers => Color::Green,
            Self::Confirm => Color::DarkGray,
            Self::FlagNonCompilation => Color::Yellow,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Confirm => ctx.has_outliers,
            Self::FlagNonCompilation | Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> AlbumArtistAction {
        match self {
            Self::Confirm => AlbumArtistAction::Confirm,
            Self::FlagNonCompilation => AlbumArtistAction::FlagNonCompilation,
            Self::Cancel => AlbumArtistAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Confirm => ProtocolBinding::Transaction {
                decision_key: DecisionKey::TagCanonicity {
                    tag_name: String::new(),
                    cluster_index: ctx.current_cluster_index,
                },
                label: "Confirm album artist".into(),
            },
            Self::FlagNonCompilation => ProtocolBinding::Transaction {
                decision_key: DecisionKey::TagCanonicity {
                    tag_name: String::new(),
                    cluster_index: ctx.current_cluster_index,
                },
                label: "Flag as non-compilation".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::views::canonicity_compound::{
        CanonicityCluster, OutlierVariant, ResolutionFileInfo, TagCanonicityResolutionData,
    };

    fn make_file(name: &str) -> ResolutionFileInfo {
        ResolutionFileInfo {
            inode: 1,
            display_name: name.to_string(),
        }
    }

    fn make_cluster(num_variants: usize, files_per_variant: usize) -> CanonicityCluster {
        let outlier_variants = (0..num_variants)
            .map(|v| OutlierVariant {
                value: format!("variant_{v}"),
                files: (0..files_per_variant)
                    .map(|f| make_file(&format!("file_{v}_{f}.flac")))
                    .collect(),
            })
            .collect();
        CanonicityCluster {
            signal_key: "album_artist::cluster_0".to_string(),
            canonical_candidate: "Various Artists".to_string(),
            canonical_count: 15,
            outlier_variants,
            default_canonical: None,
        }
    }

    fn make_test_data(num_clusters: usize) -> TagCanonicityResolutionData {
        TagCanonicityResolutionData {
            tag_name: "album_artist".to_string(),
            clusters: (0..num_clusters)
                .map(|_| make_cluster(3, 2))
                .collect(),
        }
    }

    // -- GroupNavigation tests --

    #[test]
    fn group_nav_zero_clusters() {
        let data = AlbumArtistData::new(make_test_data(0));
        assert_eq!(data.group_count(), 0);
        assert!(!data.has_next());
        assert!(!data.has_prev());
    }

    #[test]
    fn group_nav_three_clusters_boundaries() {
        let mut data = AlbumArtistData::new(make_test_data(3));
        // At start
        assert_eq!(data.group_count(), 3);
        assert_eq!(data.current_group(), 0);
        assert!(data.has_next());
        assert!(!data.has_prev());

        // Middle
        data.current_cluster = 1;
        assert!(data.has_next());
        assert!(data.has_prev());

        // End
        data.current_cluster = 2;
        assert!(!data.has_next());
        assert!(data.has_prev());
    }

    // -- ResolutionData tests --

    #[test]
    fn list_len_returns_outlier_variant_count() {
        let data = AlbumArtistData::new(make_test_data(2));
        assert_eq!(data.list_len(), 3);
    }

    #[test]
    fn list_len_empty_clusters() {
        let data = AlbumArtistData::new(make_test_data(0));
        assert_eq!(data.list_len(), 0);
    }

    #[test]
    fn list_len_changes_with_cluster() {
        let mut inner = make_test_data(0);
        inner.clusters.push(make_cluster(2, 1)); // cluster 0: 2 variants
        inner.clusters.push(make_cluster(5, 1)); // cluster 1: 5 variants
        let mut data = AlbumArtistData::new(inner);
        assert_eq!(data.list_len(), 2);
        data.current_cluster = 1;
        assert_eq!(data.list_len(), 5);
    }

    #[test]
    fn selected_path_returns_first_file_display_name() {
        let data = AlbumArtistData::new(make_test_data(1));
        assert_eq!(data.selected_path(0), Some("file_0_0.flac"));
    }

    #[test]
    fn selected_path_out_of_bounds() {
        let data = AlbumArtistData::new(make_test_data(1));
        assert_eq!(data.selected_path(999), None);
    }

    #[test]
    fn list_title_includes_position() {
        let data = AlbumArtistData::new(make_test_data(3));
        let title = data.list_title();
        assert!(title.contains("album_artist"));
        assert!(title.contains("1/3"));
    }

    #[test]
    fn content_layout_is_field_above_list() {
        let data = AlbumArtistData::new(make_test_data(1));
        assert!(matches!(
            data.content_layout(),
            ContentLayout::FieldAboveList { .. }
        ));
    }

    #[test]
    fn button_ctx_reflects_data_state() {
        let data = AlbumArtistData::new(make_test_data(1));
        let ctx = data.button_ctx();
        assert!(ctx.has_outliers);
        assert_eq!(ctx.current_cluster_index, 0);

        let empty = AlbumArtistData::new(make_test_data(0));
        let ctx = empty.button_ctx();
        assert!(!ctx.has_outliers);
    }

    // -- ModalButtons tests --

    #[test]
    fn all_buttons_returned() {
        let all = AlbumArtistButton::all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], AlbumArtistButton::Confirm);
        assert_eq!(all[1], AlbumArtistButton::FlagNonCompilation);
        assert_eq!(all[2], AlbumArtistButton::Cancel);
    }

    #[test]
    fn confirm_disabled_when_no_outliers() {
        let ctx = AlbumArtistButtonCtx {
            has_outliers: false,
            current_cluster_index: 0,
        };
        assert!(!AlbumArtistButton::Confirm.enabled(&ctx));
        assert!(AlbumArtistButton::FlagNonCompilation.enabled(&ctx));
        assert!(AlbumArtistButton::Cancel.enabled(&ctx));
    }

    #[test]
    fn confirm_enabled_when_outliers_exist() {
        let ctx = AlbumArtistButtonCtx {
            has_outliers: true,
            current_cluster_index: 0,
        };
        assert!(AlbumArtistButton::Confirm.enabled(&ctx));
    }

    #[test]
    fn default_button_is_cancel() {
        assert_eq!(AlbumArtistButton::default(), AlbumArtistButton::Cancel);
    }

    #[test]
    fn actions_map_correctly() {
        let ctx = AlbumArtistButtonCtx {
            has_outliers: true,
            current_cluster_index: 0,
        };
        assert_eq!(
            AlbumArtistButton::Confirm.action(&ctx),
            AlbumArtistAction::Confirm
        );
        assert_eq!(
            AlbumArtistButton::FlagNonCompilation.action(&ctx),
            AlbumArtistAction::FlagNonCompilation
        );
        assert_eq!(
            AlbumArtistButton::Cancel.action(&ctx),
            AlbumArtistAction::Cancel
        );
    }

    #[test]
    fn labels_are_nonempty() {
        let ctx = AlbumArtistButtonCtx {
            has_outliers: true,
            current_cluster_index: 0,
        };
        for button in AlbumArtistButton::all() {
            assert!(!button.label(&ctx).is_empty());
        }
    }
}
