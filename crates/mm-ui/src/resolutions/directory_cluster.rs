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
