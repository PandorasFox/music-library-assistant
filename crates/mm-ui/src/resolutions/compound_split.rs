//! Compound Split resolution — split compound tag values into individual entries.
//!
//! Route: `/resolve/compound-split`
//! Query: `GetCompoundSplitResolutionData`
//! Data: `CompoundSplitResolutionData` (mm-meta, packed groups)
//! Mutations: tag ops per group (Confirm) or canonicalize as entity

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::canonicity_compound::CompoundSplitResolutionData;

use crate::group_navigation::GroupNavigation;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta packed type to implement `ResolutionData` + `GroupNavigation`.
pub struct CompoundSplitData {
    pub inner: CompoundSplitResolutionData,
    pub current_group: usize,
}

impl CompoundSplitData {
    pub fn new(data: CompoundSplitResolutionData) -> Self {
        Self {
            inner: data,
            current_group: 0,
        }
    }
}

impl ResolutionData for CompoundSplitData {
    type ButtonCtx = CompoundSplitButtonCtx;

    fn list_len(&self) -> usize {
        self.inner
            .groups
            .get(self.current_group)
            .map_or(0, |g| g.files.len())
    }

    fn button_ctx(&self) -> CompoundSplitButtonCtx {
        CompoundSplitButtonCtx {
            has_files: self.list_len() > 0,
            current_group_index: self.current_group,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.inner
            .groups
            .get(self.current_group)
            .and_then(|g| g.files.get(cursor))
            .map(|f| f.display_name.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FieldAboveList {
            field_height: 3,
            detail_height: 3,
        }
    }

    fn list_title(&self) -> String {
        let current = self.current_group + 1;
        let total = self.inner.groups.len();
        let compound_value = self
            .inner
            .groups
            .get(self.current_group)
            .map(|g| g.compound_value.as_str())
            .unwrap_or("?");
        format!(" {} ({}/{}) ", compound_value, current, total)
    }

    fn empty_message(&self) -> &'static str {
        "No compound split groups"
    }
}

impl GroupNavigation for CompoundSplitData {
    fn group_count(&self) -> usize {
        self.inner.groups.len()
    }

    fn current_group(&self) -> usize {
        self.current_group
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for compound split modals.
pub type CompoundSplitState = ResolutionState<CompoundSplitData, CompoundSplitButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompoundSplitAction {
    Confirm,
    Canonicalize,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompoundSplitButton {
    Confirm,
    Canonicalize,
    #[default]
    Cancel,
}

pub struct CompoundSplitButtonCtx {
    pub has_files: bool,
    pub current_group_index: usize,
}

impl ModalButtons for CompoundSplitButton {
    type Context = CompoundSplitButtonCtx;
    type Action = CompoundSplitAction;

    fn all() -> &'static [Self] {
        &[Self::Confirm, Self::Canonicalize, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Confirm => "Confirm".into(),
            Self::Canonicalize => "Mark as Entity".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Confirm if ctx.has_files => Color::Green,
            Self::Confirm => Color::DarkGray,
            Self::Canonicalize => Color::Yellow,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Confirm => ctx.has_files,
            Self::Canonicalize | Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> CompoundSplitAction {
        match self {
            Self::Confirm => CompoundSplitAction::Confirm,
            Self::Canonicalize => CompoundSplitAction::Canonicalize,
            Self::Cancel => CompoundSplitAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Confirm => ProtocolBinding::Transaction {
                decision_key: DecisionKey::CompoundSplitSafe {
                    tag_name: String::new(),
                    cluster_index: ctx.current_group_index,
                },
                label: "Confirm compound split".into(),
            },
            Self::Canonicalize => ProtocolBinding::Transaction {
                decision_key: DecisionKey::CompoundSplitSafe {
                    tag_name: String::new(),
                    cluster_index: ctx.current_group_index,
                },
                label: "Mark as entity".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::views::canonicity_compound::{CompoundSplitCluster, ResolutionFileInfo};

    fn make_file(name: &str) -> ResolutionFileInfo {
        ResolutionFileInfo {
            inode: 1,
            display_name: name.to_string(),
        }
    }

    fn make_group(num_files: usize) -> CompoundSplitCluster {
        CompoundSplitCluster {
            tag_name: "genre".to_string(),
            compound_value: "Rock; Metal".to_string(),
            split_parts: vec!["Rock".to_string(), "Metal".to_string()],
            matching_parts: vec!["Rock".to_string()],
            files: (0..num_files)
                .map(|i| make_file(&format!("track_{i}.flac")))
                .collect(),
        }
    }

    fn make_test_data(num_groups: usize) -> CompoundSplitResolutionData {
        CompoundSplitResolutionData {
            groups: (0..num_groups).map(|_| make_group(4)).collect(),
        }
    }

    // -- GroupNavigation tests --

    #[test]
    fn group_nav_zero_groups() {
        let data = CompoundSplitData::new(make_test_data(0));
        assert_eq!(data.group_count(), 0);
        assert!(!data.has_next());
        assert!(!data.has_prev());
    }

    #[test]
    fn group_nav_three_groups_boundaries() {
        let mut data = CompoundSplitData::new(make_test_data(3));
        // At start
        assert_eq!(data.group_count(), 3);
        assert_eq!(data.current_group(), 0);
        assert!(data.has_next());
        assert!(!data.has_prev());

        // Middle
        data.current_group = 1;
        assert!(data.has_next());
        assert!(data.has_prev());

        // End
        data.current_group = 2;
        assert!(!data.has_next());
        assert!(data.has_prev());
    }

    // -- ResolutionData tests --

    #[test]
    fn list_len_returns_file_count() {
        let data = CompoundSplitData::new(make_test_data(1));
        // Each group has 4 files
        assert_eq!(data.list_len(), 4);
    }

    #[test]
    fn list_len_empty_groups() {
        let data = CompoundSplitData::new(make_test_data(0));
        assert_eq!(data.list_len(), 0);
    }

    #[test]
    fn list_len_changes_with_group() {
        let mut inner = CompoundSplitResolutionData { groups: vec![] };
        inner.groups.push(make_group(2)); // group 0: 2 files
        inner.groups.push(make_group(7)); // group 1: 7 files
        let mut data = CompoundSplitData::new(inner);
        assert_eq!(data.list_len(), 2);
        data.current_group = 1;
        assert_eq!(data.list_len(), 7);
    }

    #[test]
    fn selected_path_returns_file_display_name() {
        let data = CompoundSplitData::new(make_test_data(1));
        assert_eq!(data.selected_path(0), Some("track_0.flac"));
        assert_eq!(data.selected_path(1), Some("track_1.flac"));
    }

    #[test]
    fn selected_path_out_of_bounds() {
        let data = CompoundSplitData::new(make_test_data(1));
        assert_eq!(data.selected_path(999), None);
    }

    #[test]
    fn list_title_includes_compound_value_and_position() {
        let data = CompoundSplitData::new(make_test_data(3));
        let title = data.list_title();
        assert!(title.contains("Rock; Metal"));
        assert!(title.contains("1/3"));
    }

    #[test]
    fn content_layout_is_field_above_list() {
        let data = CompoundSplitData::new(make_test_data(1));
        assert!(matches!(
            data.content_layout(),
            ContentLayout::FieldAboveList { .. }
        ));
    }

    #[test]
    fn button_ctx_reflects_data_state() {
        let data = CompoundSplitData::new(make_test_data(1));
        let ctx = data.button_ctx();
        assert!(ctx.has_files);
        assert_eq!(ctx.current_group_index, 0);

        let empty = CompoundSplitData::new(make_test_data(0));
        let ctx = empty.button_ctx();
        assert!(!ctx.has_files);
    }

    // -- ModalButtons tests --

    #[test]
    fn all_buttons_returned() {
        let all = CompoundSplitButton::all();
        assert_eq!(all.len(), 3);
        assert_eq!(all[0], CompoundSplitButton::Confirm);
        assert_eq!(all[1], CompoundSplitButton::Canonicalize);
        assert_eq!(all[2], CompoundSplitButton::Cancel);
    }

    #[test]
    fn confirm_disabled_when_no_files() {
        let ctx = CompoundSplitButtonCtx {
            has_files: false,
            current_group_index: 0,
        };
        assert!(!CompoundSplitButton::Confirm.enabled(&ctx));
        assert!(CompoundSplitButton::Canonicalize.enabled(&ctx));
        assert!(CompoundSplitButton::Cancel.enabled(&ctx));
    }

    #[test]
    fn confirm_enabled_when_files_exist() {
        let ctx = CompoundSplitButtonCtx {
            has_files: true,
            current_group_index: 0,
        };
        assert!(CompoundSplitButton::Confirm.enabled(&ctx));
    }

    #[test]
    fn default_button_is_cancel() {
        assert_eq!(CompoundSplitButton::default(), CompoundSplitButton::Cancel);
    }

    #[test]
    fn actions_map_correctly() {
        let ctx = CompoundSplitButtonCtx {
            has_files: true,
            current_group_index: 0,
        };
        assert_eq!(
            CompoundSplitButton::Confirm.action(&ctx),
            CompoundSplitAction::Confirm
        );
        assert_eq!(
            CompoundSplitButton::Canonicalize.action(&ctx),
            CompoundSplitAction::Canonicalize
        );
        assert_eq!(
            CompoundSplitButton::Cancel.action(&ctx),
            CompoundSplitAction::Cancel
        );
    }

    #[test]
    fn labels_are_nonempty() {
        let ctx = CompoundSplitButtonCtx {
            has_files: true,
            current_group_index: 0,
        };
        for button in CompoundSplitButton::all() {
            assert!(!button.label(&ctx).is_empty());
        }
    }
}
