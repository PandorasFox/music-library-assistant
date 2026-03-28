//! Directory Cluster resolution — cross-source overlap clusters.
//!
//! Route: `/resolve/directory-clusters`
//! Query: `GetDirectoryClusterData`
//! Data: `DirectoryClusterModalData` (mm-meta)
//! Mutations: StashFromZone + DropFromIndex per cluster, or MarkExpected

use std::borrow::Cow;
use std::collections::BTreeSet;
use std::path::PathBuf;

use ratatui::style::{Color, Style};

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::file_ops::StashFromZoneMutation;
use mm_meta::mutations::indexing::{DropFromIndexMutation, EmitExpectedOverlapMutation};
use mm_meta::mutations::Mutation;
use mm_meta::paths::PathResolver;
use mm_meta::views::cluster_deploy::DirectoryClusterModalData;

use crate::group_navigation::GroupNavigation;
use crate::input::InputAction;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};
use crate::rich_text::{RichBlock, RichSpan};
use crate::standard_list::{ListEntry, ListInputResult};
use crate::wizard::{WizardItem, WizardOffer};

// ============================================================================
// ClusterDirItem — list item for a directory within a cluster
// ============================================================================

/// Display wrapper for a directory within a cluster.
///
/// Implements `WizardItem` (detail pane) and `ListEntry` (radio selection).
/// Shared between TUI and web clients.
pub struct ClusterDirItem {
    pub path_suffix: String,
    pub format_summary: String,
    pub file_count: usize,
    pub paths: Vec<String>,
    pub can_stash: bool,
}

impl WizardItem for ClusterDirItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        let mut content = vec![RichBlock::Paragraph(vec![RichSpan::new(
            &format!("{} \u{2014} {}", self.format_summary, self.file_count),
            Style::default().fg(Color::Cyan),
        )])];

        for path in &self.paths {
            content.push(RichBlock::Paragraph(vec![RichSpan::new(
                path,
                Style::default().fg(Color::White),
            )]));
        }

        Some(WizardOffer::Pane {
            title: self.path_suffix.clone(),
            content,
        })
    }
}

impl ListEntry for ClusterDirItem {
    type Action = ();
    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<()> {
        None
    }
    fn is_toggleable(&self) -> bool {
        self.can_stash
    }
}

/// Build items from a cluster's directories.
pub fn build_dir_items(
    cluster: &mm_meta::views::cluster_deploy::DirectoryClusterEntry,
) -> Vec<ClusterDirItem> {
    cluster
        .directories
        .iter()
        .map(|d| ClusterDirItem {
            path_suffix: d.path_suffix.clone(),
            format_summary: d.format_summary.clone(),
            file_count: d.paths.len(),
            paths: d.paths.clone(),
            can_stash: d.can_stash_dupes,
        })
        .collect()
}

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData` + `GroupNavigation`.
pub struct DirectoryClusterData {
    pub inner: DirectoryClusterModalData,
    pub current_cluster: usize,
    /// Radio-selected directory index (the one that will be stashed).
    pub stash_selection: Option<usize>,
    pub wizard_cache: crate::wizard_detail::WizardDetailCache,
}

impl DirectoryClusterData {
    pub fn new(data: DirectoryClusterModalData) -> Self {
        Self {
            inner: data,
            current_cluster: 0,
            stash_selection: None,
            wizard_cache: crate::wizard_detail::WizardDetailCache::new(
                mm_meta::db_types::Zone::Corpus,
            ),
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
            has_stash_selection: self.stash_selection.is_some(),
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
// Input handling — group navigation + StandardList + frame
// ============================================================================

impl DirectoryClusterState {
    /// Handle input with group navigation (Tab/Shift+Tab) and StandardList
    /// integration (Z for wizard, Space for radio selection, cursor movement).
    ///
    /// Call this instead of the generic `handle_input` which misses these.
    pub fn handle_input_with_items(&mut self, action: &InputAction) -> Option<DirectoryClusterAction> {
        use crate::group_navigation::{try_group_navigate, GroupInputResult};
        use crate::modal_frame::{FrameInputResult, ModalFrameCore};

        // 1. Group navigation (Tab / Shift+Tab to browse clusters)
        if let Some(result) = try_group_navigate(&self.data, action) {
            match result {
                GroupInputResult::NavigateNext => self.navigate_group(1),
                GroupInputResult::NavigatePrev => self.navigate_group(-1),
                _ => {}
            }
            return None;
        }

        // 2. StandardList input — only when list pane has focus.
        //    When buttons have focus, skip straight to frame input so that
        //    Confirm reaches the button handler instead of being swallowed
        //    by items whose on_confirm returns None.
        if self.frame.focus_pane == crate::geometry::FocusPane::List {
            let items = self
                .data
                .inner
                .clusters
                .get(self.data.current_cluster)
                .map(build_dir_items)
                .unwrap_or_default();

            match self.list.handle_input(action, &items) {
                ListInputResult::Consumed | ListInputResult::CursorMoved => return None,
                ListInputResult::Toggled => {
                    // Sync radio selection from StandardList's selected set
                    self.data.stash_selection = self.list.selected.iter().next().copied();
                    return None;
                }
                ListInputResult::Confirm(_) => return None, // items return None on confirm
                ListInputResult::Unhandled => {} // fall through to frame input
            }
        }

        // 3. Frame input (focus cycling, buttons, escape)
        match self.handle_frame_input(action) {
            FrameInputResult::Action(a) => Some(a),
            FrameInputResult::Consumed | FrameInputResult::Unhandled => None,
        }
    }

    /// Navigate to a different cluster by delta.
    fn navigate_group(&mut self, delta: isize) {
        let new_idx = self.data.current_cluster as isize + delta;
        let total = self.data.inner.clusters.len();
        if new_idx >= 0 && (new_idx as usize) < total {
            self.data.current_cluster = new_idx as usize;
            self.reset_list();
            self.data.stash_selection = None;
        }
    }
}

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for DirectoryClusterState {
    type Action = DirectoryClusterAction;

    fn dispatch(
        &self,
        action: DirectoryClusterAction,
        resolver: &PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            DirectoryClusterAction::Stash => {
                let cluster = match self.data.inner.clusters.get(self.data.current_cluster) {
                    Some(c) => c,
                    None => return DispatchResult::Handled,
                };
                // Use the radio-selected directory, not cursor position
                let dir_idx = match self.data.stash_selection {
                    Some(idx) => idx,
                    None => return DispatchResult::Handled,
                };
                let dir = match cluster.directories.get(dir_idx) {
                    Some(d) => d,
                    None => return DispatchResult::Handled,
                };

                let mutations = stash_directory_mutations(dir, resolver);
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = self.data.button_ctx();
                let key = DirectoryClusterButton::Stash
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: "Stash overlapping directory".into(),
                    mutations,
                }
            }
            DirectoryClusterAction::MarkExpected => {
                let cluster = match self.data.inner.clusters.get(self.data.current_cluster) {
                    Some(c) => c,
                    None => return DispatchResult::Handled,
                };

                let parts: Vec<&str> = cluster.cluster_key.splitn(2, '|').collect();
                let source_a = parts.first().unwrap_or(&"").to_string();
                let source_b = parts.get(1).unwrap_or(&"").to_string();

                let mutation =
                    Mutation::EmitExpectedOverlap(EmitExpectedOverlapMutation { source_a, source_b });

                let ctx = self.data.button_ctx();
                let key = DirectoryClusterButton::MarkExpected
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: "Mark expected overlap".into(),
                    mutations: vec![mutation],
                }
            }
            DirectoryClusterAction::Skip => DispatchResult::Skip,
            DirectoryClusterAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn advance(&mut self) -> bool {
        if self.data.current_cluster + 1 < self.data.inner.clusters.len() {
            self.data.current_cluster += 1;
            self.reset_list();
            self.data.stash_selection = None;
            true
        } else {
            false
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Directory overlap cluster resolution cancelled"
    }
}

/// Build stash + drop mutations for a directory.
fn stash_directory_mutations(
    dir: &mm_meta::views::cluster_deploy::DirectoryGroupEntry,
    resolver: &PathResolver,
) -> Vec<Mutation> {
    use mm_meta::db_types::Zone;

    if !dir.can_stash_dupes {
        return Vec::new();
    }

    let mut mutations = Vec::new();
    for (idx, corpus_path) in dir.paths.iter().enumerate() {
        let abs_path = resolver.resolve_for_zone(Zone::Corpus, std::path::Path::new(corpus_path));
        mutations.push(Mutation::StashFromZone(StashFromZoneMutation {
            path: abs_path,
            stash_name: "overlaps".to_string(),
        }));

        let inode = dir.inodes.get(idx).copied();
        mutations.push(Mutation::DropFromIndex(DropFromIndexMutation {
            path: PathBuf::from(corpus_path),
            inode,
            zone: Some("corpus".to_string()),
        }));
    }
    mutations
}

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DirectoryClusterAction {
    Stash,
    MarkExpected,
    Skip,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DirectoryClusterButton {
    Stash,
    Skip,
    MarkExpected,
    #[default]
    Cancel,
}

pub struct DirectoryClusterButtonCtx {
    pub has_directories: bool,
    /// Whether a stashable directory is radio-selected.
    pub has_stash_selection: bool,
    pub cluster_index: usize,
}

impl ModalButtons for DirectoryClusterButton {
    type Context = DirectoryClusterButtonCtx;
    type Action = DirectoryClusterAction;

    fn all() -> &'static [Self] {
        &[Self::Stash, Self::Skip, Self::MarkExpected, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Stash => "Stash Selected".into(),
            Self::Skip => "Skip".into(),
            Self::MarkExpected => "Mark Expected".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Stash if ctx.has_stash_selection => Color::Yellow,
            Self::Stash => Color::DarkGray,
            Self::Skip => Color::White,
            Self::MarkExpected if ctx.has_directories => Color::Magenta,
            Self::MarkExpected => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Stash => ctx.has_stash_selection,
            Self::Skip => ctx.has_directories,
            Self::MarkExpected => ctx.has_directories,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> DirectoryClusterAction {
        match self {
            Self::Stash => DirectoryClusterAction::Stash,
            Self::Skip => DirectoryClusterAction::Skip,
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
            Self::Skip | Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::views::cluster_deploy::{DirectoryClusterEntry, DirectoryGroupEntry};

    fn make_dir_entry(path: &str, can_stash: bool) -> DirectoryGroupEntry {
        DirectoryGroupEntry {
            path_suffix: path.to_string(),
            inodes: vec![1, 2],
            paths: vec![
                format!("{path}/track_1.flac"),
                format!("{path}/track_2.flac"),
            ],
            format_summary: "FLAC (2)".to_string(),
            can_stash_dupes: can_stash,
        }
    }

    fn make_cluster(key: &str, num_dirs: usize) -> DirectoryClusterEntry {
        let directories = (0..num_dirs)
            .map(|i| make_dir_entry(&format!("web/releases/source_{i}"), true))
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
        assert_eq!(data.selected_path(0), Some("web/releases/source_0"));
        assert_eq!(data.selected_path(2), Some("web/releases/source_2"));
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

    // -- ButtonCtx tests --

    #[test]
    fn stash_enabled_only_with_selection() {
        let ctx = DirectoryClusterButtonCtx {
            has_directories: true,
            has_stash_selection: false,
            cluster_index: 0,
        };
        assert!(!DirectoryClusterButton::Stash.enabled(&ctx));

        let ctx = DirectoryClusterButtonCtx {
            has_directories: true,
            has_stash_selection: true,
            cluster_index: 0,
        };
        assert!(DirectoryClusterButton::Stash.enabled(&ctx));
    }

    #[test]
    fn skip_and_mark_expected_enabled_when_has_directories() {
        let ctx = DirectoryClusterButtonCtx {
            has_directories: true,
            has_stash_selection: false,
            cluster_index: 0,
        };
        assert!(DirectoryClusterButton::Skip.enabled(&ctx));
        assert!(DirectoryClusterButton::MarkExpected.enabled(&ctx));
    }

    #[test]
    fn skip_and_mark_expected_disabled_when_empty() {
        let ctx = DirectoryClusterButtonCtx {
            has_directories: false,
            has_stash_selection: false,
            cluster_index: 0,
        };
        assert!(!DirectoryClusterButton::Skip.enabled(&ctx));
        assert!(!DirectoryClusterButton::MarkExpected.enabled(&ctx));
    }

    #[test]
    fn cancel_always_enabled() {
        for has_dirs in [true, false] {
            let ctx = DirectoryClusterButtonCtx {
                has_directories: has_dirs,
                has_stash_selection: false,
                cluster_index: 0,
            };
            assert!(DirectoryClusterButton::Cancel.enabled(&ctx));
        }
    }

    #[test]
    fn actions_return_correct_variants() {
        let ctx = DirectoryClusterButtonCtx {
            has_directories: true,
            has_stash_selection: true,
            cluster_index: 0,
        };
        assert_eq!(
            DirectoryClusterButton::Stash.action(&ctx),
            DirectoryClusterAction::Stash
        );
        assert_eq!(
            DirectoryClusterButton::Skip.action(&ctx),
            DirectoryClusterAction::Skip
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
        assert!(!ctx.has_stash_selection);
        assert_eq!(ctx.cluster_index, 0);

        let empty = DirectoryClusterData::new(make_cluster_data(0, 0));
        let ctx = empty.button_ctx();
        assert!(!ctx.has_directories);
    }

    #[test]
    fn button_ctx_with_stash_selection() {
        let mut data = DirectoryClusterData::new(make_cluster_data(1, 3));
        data.stash_selection = Some(1);
        let ctx = data.button_ctx();
        assert!(ctx.has_stash_selection);
    }

    // -- ClusterDirItem tests --

    #[test]
    fn dir_item_toggleable_when_can_stash() {
        let item = ClusterDirItem {
            path_suffix: "test".into(),
            format_summary: "FLAC (2)".into(),
            file_count: 2,
            paths: vec![],
            can_stash: true,
        };
        assert!(item.is_toggleable());
    }

    #[test]
    fn dir_item_not_toggleable_when_cannot_stash() {
        let item = ClusterDirItem {
            path_suffix: "test".into(),
            format_summary: "FLAC (2)".into(),
            file_count: 2,
            paths: vec![],
            can_stash: false,
        };
        assert!(!item.is_toggleable());
    }

    // -- Group navigation via handle_input_with_items --

    #[test]
    fn tab_advances_cluster() {
        use crate::standard_list::StandardListConfig;
        let data = DirectoryClusterData::new(make_cluster_data(3, 2));
        let mut state = DirectoryClusterState::with_list_config(
            data,
            StandardListConfig {
                radio_select: true,
                ..Default::default()
            },
        );
        assert_eq!(state.data.current_cluster, 0);

        state.handle_input_with_items(&InputAction::CycleNext);
        assert_eq!(state.data.current_cluster, 1);

        state.handle_input_with_items(&InputAction::CycleNext);
        assert_eq!(state.data.current_cluster, 2);

        // At last cluster — stays
        state.handle_input_with_items(&InputAction::CycleNext);
        assert_eq!(state.data.current_cluster, 2);
    }

    #[test]
    fn shift_tab_retreats_cluster() {
        use crate::standard_list::StandardListConfig;
        let data = DirectoryClusterData::new(make_cluster_data(3, 2));
        let mut state = DirectoryClusterState::with_list_config(
            data,
            StandardListConfig {
                radio_select: true,
                ..Default::default()
            },
        );
        state.data.current_cluster = 2;

        state.handle_input_with_items(&InputAction::CyclePrev);
        assert_eq!(state.data.current_cluster, 1);

        state.handle_input_with_items(&InputAction::CyclePrev);
        assert_eq!(state.data.current_cluster, 0);

        // At first cluster — stays
        state.handle_input_with_items(&InputAction::CyclePrev);
        assert_eq!(state.data.current_cluster, 0);
    }

    #[test]
    fn group_navigation_clears_stash_selection() {
        use crate::standard_list::StandardListConfig;
        let data = DirectoryClusterData::new(make_cluster_data(3, 2));
        let mut state = DirectoryClusterState::with_list_config(
            data,
            StandardListConfig {
                radio_select: true,
                ..Default::default()
            },
        );
        state.data.stash_selection = Some(1);
        state.list.selected.insert(1);

        state.handle_input_with_items(&InputAction::CycleNext);
        assert!(state.data.stash_selection.is_none());
        assert!(state.list.selected.is_empty());
    }
}
