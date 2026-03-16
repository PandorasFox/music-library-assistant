//! Tag Canonicity resolution — squash tag variants to a canonical spelling.
//!
//! Route: `/resolve/tag-canonicity`
//! Query: `GetTagCanonicityResolutionData`
//! Data: `TagCanonicityResolutionData` (mm-meta, packed clusters)
//! Mutations: canonical tag emit per cluster (Confirm) or flag-as-canonical

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
// CanonicityMode — distinguishes shared UI skeleton modes
// ============================================================================

/// Distinguishes tag canonicity modes that share the same UI skeleton.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CanonicityMode {
    /// Squashing tag variants to a canonical spelling (e.g., DragonForce vs Dragonforce)
    TagCanonicity,
    /// Resolving inconsistent album artist across a release
    InconsistentAlbumArtist,
}

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta packed type to implement `ResolutionData` + `GroupNavigation`.
pub struct TagCanonicityData {
    pub inner: TagCanonicityResolutionData,
    pub current_cluster: usize,
    pub zone: mm_meta::db_types::Zone,
    pub mode: CanonicityMode,
    pub wizard_cache: crate::wizard_detail::WizardDetailCache,
}

impl TagCanonicityData {
    pub fn new(
        data: TagCanonicityResolutionData,
        zone: mm_meta::db_types::Zone,
        mode: CanonicityMode,
    ) -> Self {
        Self {
            inner: data,
            current_cluster: 0,
            zone,
            mode,
            wizard_cache: crate::wizard_detail::WizardDetailCache::new(zone),
        }
    }

    /// Test helper with defaults.
    #[cfg(test)]
    fn test_new(data: TagCanonicityResolutionData) -> Self {
        Self::new(data, mm_meta::db_types::Zone::Corpus, CanonicityMode::TagCanonicity)
    }
}

impl ResolutionData for TagCanonicityData {
    type ButtonCtx = CanonicityButtonCtx;

    fn list_len(&self) -> usize {
        self.inner
            .clusters
            .get(self.current_cluster)
            .map_or(0, |c| c.variants.len())
    }

    fn button_ctx(&self) -> CanonicityButtonCtx {
        CanonicityButtonCtx {
            has_variants: self.list_len() > 0,
            current_cluster_index: self.current_cluster,
            mode: self.mode,
            tag_name: self.inner.tag_name.clone(),
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.inner
            .clusters
            .get(self.current_cluster)
            .and_then(|c| c.variants.get(cursor))
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

impl GroupNavigation for TagCanonicityData {
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

/// Concrete resolution state for tag canonicity modals.
pub type TagCanonicityState = ResolutionState<TagCanonicityData, CanonicityButton>;

// ============================================================================
// ViewState (ResolutionState + DecisionField)
// ============================================================================

/// Bundled view state for tag canonicity — composes ResolutionState with DecisionField.
pub struct TagCanonicityViewState {
    pub state: TagCanonicityState,
    pub field: crate::decision_field::DecisionField,
}

impl TagCanonicityViewState {
    pub fn new(data: TagCanonicityData, field_label: &str, prefill: &str) -> Self {
        Self {
            state: TagCanonicityState::new(data),
            field: crate::decision_field::DecisionField::new(field_label).with_value(prefill),
        }
    }

    /// Handle input via WithDecisionField composition.
    pub fn handle_input(
        &mut self,
        action: &crate::input::InputAction,
    ) -> Option<CanonicityAction> {
        use crate::decision_field::WithDecisionField;
        use crate::group_navigation::{try_group_navigate, GroupInputResult};
        use crate::modal_frame::{FrameInputResult, ModalFrameCore};

        // Group navigation first
        if let Some(result) = try_group_navigate(&self.state.data, action) {
            return match result {
                GroupInputResult::NavigateNext => {
                    self.navigate_group(1);
                    None
                }
                GroupInputResult::NavigatePrev => {
                    self.navigate_group(-1);
                    None
                }
                GroupInputResult::Consumed => None,
                GroupInputResult::Action(_) | GroupInputResult::Unhandled => None,
            };
        }

        // Delegate to frame (handles focus cycling, buttons, field, list nav)
        let mut composed = WithDecisionField::new(&mut self.state, &mut self.field);
        match composed.handle_frame_input(action) {
            FrameInputResult::Action(a) => Some(a),
            FrameInputResult::Consumed | FrameInputResult::Unhandled => None,
        }
    }

    /// Navigate to a different cluster (for Tab/Shift-Tab group cycling).
    fn navigate_group(&mut self, delta: isize) {
        let new_idx = self.state.data.current_cluster as isize + delta;
        let total = self.state.data.inner.clusters.len();
        if new_idx >= 0 && (new_idx as usize) < total {
            self.state.data.current_cluster = new_idx as usize;
            self.state.reset_list();
            let prefill = self.state.data.inner.clusters
                .get(self.state.data.current_cluster)
                .and_then(|c| c.suggested_canonical.as_deref())
                .unwrap_or("");
            self.field.set_value(prefill);
        }
    }

    /// Handle mouse click.
    pub fn handle_click(&mut self, x: u16, y: u16) -> Option<CanonicityAction> {
        self.state.handle_click(x, y)
    }

    /// Selected path for status bar.
    pub fn selected_path(&self) -> Option<&str> {
        self.state.selected_path()
    }
}

impl super::dispatch::Dispatchable for TagCanonicityViewState {
    type Action = CanonicityAction;

    fn dispatch(
        &self,
        action: CanonicityAction,
        _resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;
        use mm_meta::mutations::{tag_edit::ApplyTagOpsMutation, Mutation, TagOp};

        let data = &self.state.data;

        match action {
            CanonicityAction::Confirm => {
                let canonical_value = self.field.value().trim().to_string();
                if canonical_value.is_empty() {
                    return DispatchResult::Handled;
                }

                let cluster = match data.inner.clusters.get(data.current_cluster) {
                    Some(c) => c,
                    None => return DispatchResult::Handled,
                };

                let mut ops = Vec::new();
                for variant in &cluster.variants {
                    for file in &variant.files {
                        ops.push(TagOp::replace_tag(
                            file.inode,
                            &data.inner.tag_name,
                            &variant.value,
                            &canonical_value,
                        ));
                    }
                }

                if ops.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = data.button_ctx();
                let key = CanonicityButton::Confirm
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: format!("Canonicalize {}", data.inner.tag_name),
                    mutations: vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                        ops,
                        zone: data.zone,
                    })],
                }
            }
            CanonicityAction::FlagCanonical => {
                let cluster = match data.inner.clusters.get(data.current_cluster) {
                    Some(c) => c,
                    None => return DispatchResult::Handled,
                };

                let ctx = data.button_ctx();
                let key = CanonicityButton::FlagCanonical
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                match data.mode {
                    CanonicityMode::InconsistentAlbumArtist => {
                        // Flag as non-compilation: add COMPILATION=0 to all files
                        let ops: Vec<TagOp> = cluster
                            .variants
                            .iter()
                            .flat_map(|v| &v.files)
                            .map(|f| TagOp::add_tag(f.inode, "COMPILATION", "0"))
                            .collect();

                        if ops.is_empty() {
                            return DispatchResult::Handled;
                        }

                        DispatchResult::Stage {
                            key,
                            label: "Flag non-compilation".into(),
                            mutations: vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
                                ops,
                                zone: data.zone,
                            })],
                        }
                    }
                    CanonicityMode::TagCanonicity => {
                        // Flag each variant + canonical candidate as canonical
                        use mm_meta::mutations::indexing::EmitCanonicalTagMutation;

                        let mut mutations: Vec<Mutation> = cluster
                            .variants
                            .iter()
                            .map(|v| {
                                Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
                                    tag_name: data.inner.tag_name.clone(),
                                    canonical_value: v.value.clone(),
                                })
                            })
                            .collect();

                        mutations.push(Mutation::EmitCanonicalTag(EmitCanonicalTagMutation {
                            tag_name: data.inner.tag_name.clone(),
                            canonical_value: cluster
                                .suggested_canonical
                                .clone()
                                .unwrap_or_default(),
                        }));

                        DispatchResult::Stage {
                            key,
                            label: "Flag canonical".into(),
                            mutations,
                        }
                    }
                }
            }
            CanonicityAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn advance(&mut self) -> bool {
        if self.state.data.current_cluster + 1 < self.state.data.inner.clusters.len() {
            self.state.data.current_cluster += 1;
            self.state.reset_list();
            let prefill = self.state.data.inner.clusters
                .get(self.state.data.current_cluster)
                .and_then(|c| c.suggested_canonical.as_deref())
                .unwrap_or("");
            self.field.set_value(prefill);
            true
        } else {
            false
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Tag canonicity resolution cancelled"
    }
}

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CanonicityAction {
    Confirm,
    FlagCanonical,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CanonicityButton {
    Confirm,
    FlagCanonical,
    #[default]
    Cancel,
}

pub struct CanonicityButtonCtx {
    pub has_variants: bool,
    pub current_cluster_index: usize,
    /// Which canonicity mode — controls button labels and semantics.
    pub mode: CanonicityMode,
    /// Tag name for DecisionKey construction (e.g. "ARTIST", "ALBUMARTIST").
    pub tag_name: String,
}

impl ModalButtons for CanonicityButton {
    type Context = CanonicityButtonCtx;
    type Action = CanonicityAction;

    fn all() -> &'static [Self] {
        &[Self::Confirm, Self::FlagCanonical, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Confirm => "Confirm".into(),
            Self::FlagCanonical => match ctx.mode {
                CanonicityMode::InconsistentAlbumArtist => "Not a Compilation".into(),
                CanonicityMode::TagCanonicity => "Flag Canonical".into(),
            },
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Confirm if ctx.has_variants => Color::Green,
            Self::Confirm => Color::DarkGray,
            Self::FlagCanonical => Color::Yellow,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Confirm => ctx.has_variants,
            Self::FlagCanonical | Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> CanonicityAction {
        match self {
            Self::Confirm => CanonicityAction::Confirm,
            Self::FlagCanonical => CanonicityAction::FlagCanonical,
            Self::Cancel => CanonicityAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Confirm => ProtocolBinding::Transaction {
                decision_key: DecisionKey::TagCanonicity {
                    tag_name: ctx.tag_name.clone(),
                    cluster_index: ctx.current_cluster_index,
                },
                label: "Confirm tag canonicity".into(),
            },
            Self::FlagCanonical => ProtocolBinding::Transaction {
                decision_key: DecisionKey::TagCanonicity {
                    tag_name: ctx.tag_name.clone(),
                    cluster_index: ctx.current_cluster_index,
                },
                label: "Flag as canonical".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::views::canonicity_compound::{
        CanonicityCluster, Variant, ResolutionFileInfo,
    };

    fn make_file(name: &str) -> ResolutionFileInfo {
        ResolutionFileInfo {
            inode: 1,
            display_name: name.to_string(),
        }
    }

    fn make_cluster(num_variants: usize, files_per_variant: usize) -> CanonicityCluster {
        let variants = (0..num_variants)
            .map(|v| Variant {
                value: format!("variant_{v}"),
                files: (0..files_per_variant)
                    .map(|f| make_file(&format!("file_{v}_{f}.flac")))
                    .collect(),
            })
            .collect();
        CanonicityCluster {
            signal_key: "artist::cluster_0".to_string(),
            confirmed_canonical: None,
            suggested_canonical: Some("Variant_0".to_string()),
            variants,
        }
    }

    fn make_test_data(num_clusters: usize) -> TagCanonicityResolutionData {
        TagCanonicityResolutionData {
            tag_name: "artist".to_string(),
            clusters: (0..num_clusters)
                .map(|_| make_cluster(3, 2))
                .collect(),
        }
    }

    // -- GroupNavigation tests --

    #[test]
    fn group_nav_zero_clusters() {
        let data = TagCanonicityData::test_new(make_test_data(0));
        assert_eq!(data.group_count(), 0);
        assert!(!data.has_next());
        assert!(!data.has_prev());
    }

    #[test]
    fn group_nav_three_clusters_boundaries() {
        let mut data = TagCanonicityData::test_new(make_test_data(3));
        // At start: has_next but not has_prev
        assert_eq!(data.group_count(), 3);
        assert_eq!(data.current_group(), 0);
        assert!(data.has_next());
        assert!(!data.has_prev());

        // Middle: both
        data.current_cluster = 1;
        assert!(data.has_next());
        assert!(data.has_prev());

        // End: has_prev but not has_next
        data.current_cluster = 2;
        assert!(!data.has_next());
        assert!(data.has_prev());
    }

    // -- ResolutionData tests --

    #[test]
    fn list_len_returns_outlier_variant_count() {
        let data = TagCanonicityData::test_new(make_test_data(2));
        // Each cluster has 3 outlier variants
        assert_eq!(data.list_len(), 3);
    }

    #[test]
    fn list_len_empty_clusters() {
        let data = TagCanonicityData::test_new(make_test_data(0));
        assert_eq!(data.list_len(), 0);
    }

    #[test]
    fn list_len_changes_with_cluster() {
        // Build data with clusters of different sizes
        let mut inner = make_test_data(0);
        inner.clusters.push(make_cluster(2, 1)); // cluster 0: 2 variants
        inner.clusters.push(make_cluster(5, 1)); // cluster 1: 5 variants
        let mut data = TagCanonicityData::test_new(inner);
        assert_eq!(data.list_len(), 2);
        data.current_cluster = 1;
        assert_eq!(data.list_len(), 5);
    }

    #[test]
    fn selected_path_returns_first_file_display_name() {
        let data = TagCanonicityData::test_new(make_test_data(1));
        // cursor=0 => first variant's first file
        assert_eq!(data.selected_path(0), Some("file_0_0.flac"));
    }

    #[test]
    fn selected_path_out_of_bounds() {
        let data = TagCanonicityData::test_new(make_test_data(1));
        assert_eq!(data.selected_path(999), None);
    }

    #[test]
    fn list_title_includes_position() {
        let data = TagCanonicityData::test_new(make_test_data(3));
        let title = data.list_title();
        assert!(title.contains("artist"));
        assert!(title.contains("1/3"));
    }

    #[test]
    fn content_layout_is_field_above_list() {
        let data = TagCanonicityData::test_new(make_test_data(1));
        assert!(matches!(
            data.content_layout(),
            ContentLayout::FieldAboveList { .. }
        ));
    }

    #[test]
    fn button_ctx_reflects_data_state() {
        let data = TagCanonicityData::test_new(make_test_data(1));
        let ctx = data.button_ctx();
        assert!(ctx.has_variants);
        assert_eq!(ctx.current_cluster_index, 0);

        let empty = TagCanonicityData::test_new(make_test_data(0));
        let ctx = empty.button_ctx();
        assert!(!ctx.has_variants);
    }

    // -- ModalButtons tests --

    #[test]
    fn all_buttons_returned() {
        let all = CanonicityButton::all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], CanonicityButton::Confirm);
        assert_eq!(all[1], CanonicityButton::FlagCanonical);
        assert_eq!(all[2], CanonicityButton::Cancel);
    }

    #[test]
    fn confirm_disabled_when_no_outliers() {
        let ctx = CanonicityButtonCtx {
            has_variants: false,
            current_cluster_index: 0,
            mode: CanonicityMode::TagCanonicity,
            tag_name: "artist".into(),
        };
        assert!(!CanonicityButton::Confirm.enabled(&ctx));
        assert!(CanonicityButton::FlagCanonical.enabled(&ctx));
        assert!(CanonicityButton::Cancel.enabled(&ctx));
    }

    #[test]
    fn confirm_enabled_when_outliers_exist() {
        let ctx = CanonicityButtonCtx {
            has_variants: true,
            current_cluster_index: 0,
            mode: CanonicityMode::TagCanonicity,
            tag_name: "artist".into(),
        };
        assert!(CanonicityButton::Confirm.enabled(&ctx));
    }

    #[test]
    fn default_button_is_cancel() {
        assert_eq!(CanonicityButton::default(), CanonicityButton::Cancel);
    }

    #[test]
    fn actions_map_correctly() {
        let ctx = CanonicityButtonCtx {
            has_variants: true,
            current_cluster_index: 0,
            mode: CanonicityMode::TagCanonicity,
            tag_name: "artist".into(),
        };
        assert_eq!(
            CanonicityButton::Confirm.action(&ctx),
            CanonicityAction::Confirm
        );
        assert_eq!(
            CanonicityButton::FlagCanonical.action(&ctx),
            CanonicityAction::FlagCanonical
        );
        assert_eq!(
            CanonicityButton::Cancel.action(&ctx),
            CanonicityAction::Cancel
        );
    }

    #[test]
    fn labels_are_nonempty() {
        let ctx = CanonicityButtonCtx {
            has_variants: true,
            current_cluster_index: 0,
            mode: CanonicityMode::TagCanonicity,
            tag_name: "artist".into(),
        };
        for button in CanonicityButton::all() {
            assert!(!button.label(&ctx).is_empty());
        }
    }
}
