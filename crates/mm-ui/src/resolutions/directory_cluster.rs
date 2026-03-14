//! Directory Cluster resolution — cross-source overlap clusters.
//!
//! Route: `/resolve/directory-clusters`
//! Query: `GetDirectoryClusterData`
//! Data: `DirectoryClusterModalData` (mm-meta)
//! Mutations: StashFromZone + DropFromIndex per cluster, or MarkExpected

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::cluster_deploy::DirectoryClusterModalData;

use crate::group_navigation::GroupNavigation;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData` + `GroupNavigation`.
pub struct DirectoryClusterData {
    pub inner: DirectoryClusterModalData,
    pub current_cluster: usize,
}

impl DirectoryClusterData {
    pub fn new(data: DirectoryClusterModalData) -> Self {
        Self {
            inner: data,
            current_cluster: 0,
        }
    }
}

impl ResolutionData for DirectoryClusterData {
    type ButtonCtx = DirectoryClusterButtonCtx;

    fn list_len(&self) -> usize {
        self.inner
            .clusters
            .get(self.current_cluster)
            .map_or(0, |c| c.directories.len())
    }

    fn button_ctx(&self) -> DirectoryClusterButtonCtx {
        DirectoryClusterButtonCtx {
            has_directories: self.list_len() > 0,
            cluster_index: self.current_cluster,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.inner
            .clusters
            .get(self.current_cluster)
            .and_then(|c| c.directories.get(cursor))
            .map(|d| d.path_suffix.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 3,
        }
    }

    fn list_title(&self) -> String {
        let dir_count = self.list_len();
        let cluster_num = self.current_cluster + 1;
        let total = self.inner.clusters.len();
        let key = self
            .inner
            .clusters
            .get(self.current_cluster)
            .map(|c| c.cluster_key.as_str())
            .unwrap_or("?");
        format!(
            " {} ({} dirs) [cluster {}/{}] ",
            key, dir_count, cluster_num, total
        )
    }

    fn empty_message(&self) -> &'static str {
        "No directory clusters"
    }
}

impl GroupNavigation for DirectoryClusterData {
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

/// Concrete resolution state for directory cluster modals.
pub type DirectoryClusterState = ResolutionState<DirectoryClusterData, DirectoryClusterButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryClusterAction {
    Stash,
    MarkExpected,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectoryClusterButton {
    Stash,
    MarkExpected,
    #[default]
    Cancel,
}

pub struct DirectoryClusterButtonCtx {
    pub has_directories: bool,
    pub cluster_index: usize,
}

impl ModalButtons for DirectoryClusterButton {
    type Context = DirectoryClusterButtonCtx;
    type Action = DirectoryClusterAction;

    fn all() -> &'static [Self] {
        &[Self::Stash, Self::MarkExpected, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Stash => "Stash".into(),
            Self::MarkExpected => "Mark Expected".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Stash if ctx.has_directories => Color::Yellow,
            Self::Stash => Color::DarkGray,
            Self::MarkExpected if ctx.has_directories => Color::Magenta,
            Self::MarkExpected => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Stash | Self::MarkExpected => ctx.has_directories,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> DirectoryClusterAction {
        match self {
            Self::Stash => DirectoryClusterAction::Stash,
            Self::MarkExpected => DirectoryClusterAction::MarkExpected,
            Self::Cancel => DirectoryClusterAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Stash => ProtocolBinding::Transaction {
                decision_key: DecisionKey::DirectoryCluster {
                    cluster_index: ctx.cluster_index,
                },
                label: "Stash overlapping directory".into(),
            },
            Self::MarkExpected => ProtocolBinding::Transaction {
                decision_key: DecisionKey::DirectoryCluster {
                    cluster_index: ctx.cluster_index,
                },
                label: "Mark overlap as expected".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::views::cluster_deploy::{
        DirectoryClusterEntry, DirectoryGroupEntry,
    };

    fn make_dir_entry(path: &str) -> DirectoryGroupEntry {
        DirectoryGroupEntry {
            path_suffix: path.to_string(),
            inodes: vec![1, 2],
            paths: vec![
                format!("{path}/track_1.flac"),
                format!("{path}/track_2.flac"),
            ],
            format_summary: "FLAC (2)".to_string(),
            can_stash_dupes: true,
        }
    }

    fn make_cluster(key: &str, num_dirs: usize) -> DirectoryClusterEntry {
        let directories = (0..num_dirs)
            .map(|i| make_dir_entry(&format!("web/releases/source_{i}")))
            .collect();
        DirectoryClusterEntry {
            cluster_key: key.to_string(),
            directories,
            overlap_count: 3,
        }
    }

    fn make_cluster_data(
        num_clusters: usize,
        dirs_per_cluster: usize,
    ) -> DirectoryClusterModalData {
        let clusters = (0..num_clusters)
            .map(|i| make_cluster(&format!("src_a|src_b_{i}"), dirs_per_cluster))
            .collect();
        DirectoryClusterModalData {
            clusters,
            file_meta_cache: Default::default(),
        }
    }

    // -- GroupNavigation tests --

    #[test]
    fn zero_clusters_navigation() {
        let data = DirectoryClusterData::new(make_cluster_data(0, 0));
        assert_eq!(data.group_count(), 0);
        assert!(!data.has_next());
        assert!(!data.has_prev());
    }

    #[test]
    fn three_clusters_navigation() {
        let mut data = DirectoryClusterData::new(make_cluster_data(3, 2));
        assert_eq!(data.group_count(), 3);
        assert!(data.has_next());
        assert!(!data.has_prev());

        data.current_cluster = 1;
        assert!(data.has_next());
        assert!(data.has_prev());

        data.current_cluster = 2;
        assert!(!data.has_next());
        assert!(data.has_prev());
    }

    // -- ResolutionData tests --

    #[test]
    fn list_len_returns_directory_count() {
        let data = DirectoryClusterData::new(make_cluster_data(1, 4));
        assert_eq!(data.list_len(), 4);
    }

    #[test]
    fn list_len_zero_when_empty() {
        let data = DirectoryClusterData::new(make_cluster_data(0, 0));
        assert_eq!(data.list_len(), 0);
    }

    #[test]
    fn list_len_changes_with_current_cluster() {
        let mut inner = DirectoryClusterModalData::default();
        inner.clusters.push(make_cluster("a", 2));
        inner.clusters.push(make_cluster("b", 5));
        let mut data = DirectoryClusterData::new(inner);
        assert_eq!(data.list_len(), 2);
        data.current_cluster = 1;
        assert_eq!(data.list_len(), 5);
    }

    #[test]
    fn selected_path_returns_path_suffix() {
        let data = DirectoryClusterData::new(make_cluster_data(1, 3));
        assert_eq!(
            data.selected_path(0),
            Some("web/releases/source_0")
        );
        assert_eq!(
            data.selected_path(2),
            Some("web/releases/source_2")
        );
    }

    #[test]
    fn selected_path_out_of_bounds() {
        let data = DirectoryClusterData::new(make_cluster_data(1, 1));
        assert!(data.selected_path(99).is_none());
    }

    #[test]
    fn list_title_includes_cluster_position() {
        let data = DirectoryClusterData::new(make_cluster_data(3, 2));
        let title = data.list_title();
        assert!(title.contains("1/3"), "expected '1/3' in: {title}");
        assert!(title.contains("2 dirs"), "expected '2 dirs' in: {title}");
    }

    // -- ModalButtons tests --

    #[test]
    fn stash_and_mark_expected_enabled_when_has_directories() {
        let ctx = DirectoryClusterButtonCtx {
            has_directories: true,
            cluster_index: 0,
        };
        assert!(DirectoryClusterButton::Stash.enabled(&ctx));
        assert!(DirectoryClusterButton::MarkExpected.enabled(&ctx));
    }

    #[test]
    fn stash_and_mark_expected_disabled_when_empty() {
        let ctx = DirectoryClusterButtonCtx {
            has_directories: false,
            cluster_index: 0,
        };
        assert!(!DirectoryClusterButton::Stash.enabled(&ctx));
        assert!(!DirectoryClusterButton::MarkExpected.enabled(&ctx));
    }

    #[test]
    fn cancel_always_enabled() {
        for has_dirs in [true, false] {
            let ctx = DirectoryClusterButtonCtx {
                has_directories: has_dirs,
                cluster_index: 0,
            };
            assert!(DirectoryClusterButton::Cancel.enabled(&ctx));
        }
    }

    #[test]
    fn actions_return_correct_variants() {
        let ctx = DirectoryClusterButtonCtx {
            has_directories: true,
            cluster_index: 0,
        };
        assert_eq!(
            DirectoryClusterButton::Stash.action(&ctx),
            DirectoryClusterAction::Stash
        );
        assert_eq!(
            DirectoryClusterButton::MarkExpected.action(&ctx),
            DirectoryClusterAction::MarkExpected
        );
        assert_eq!(
            DirectoryClusterButton::Cancel.action(&ctx),
            DirectoryClusterAction::Cancel
        );
    }

    #[test]
    fn button_ctx_reflects_data_state() {
        let data = DirectoryClusterData::new(make_cluster_data(1, 3));
        let ctx = data.button_ctx();
        assert!(ctx.has_directories);
        assert_eq!(ctx.cluster_index, 0);

        let empty = DirectoryClusterData::new(make_cluster_data(0, 0));
        let ctx = empty.button_ctx();
        assert!(!ctx.has_directories);
    }
}
