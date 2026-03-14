//! Disc Extraction resolution — extract disc numbers from ALBUM or TRACKNUMBER tags.
//!
//! Route: `/resolve/disc-extraction`
//! Query: `GetDiscExtractionData`
//! Data: `DiscExtractionModalData` (mm-meta)
//! Mutations: tag writes per group (Apply) or skip

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::domain_queries::DiscExtractionModalData;

use crate::group_navigation::GroupNavigation;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData` + `GroupNavigation`.
pub struct DiscExtractionData {
    pub inner: DiscExtractionModalData,
    pub current_group: usize,
}

impl DiscExtractionData {
    pub fn new(data: DiscExtractionModalData) -> Self {
        Self {
            inner: data,
            current_group: 0,
        }
    }
}

impl ResolutionData for DiscExtractionData {
    type ButtonCtx = DiscExtractionButtonCtx;

    fn list_len(&self) -> usize {
        self.inner
            .groups
            .get(self.current_group)
            .map_or(0, |g| g.files.len())
    }

    fn button_ctx(&self) -> DiscExtractionButtonCtx {
        DiscExtractionButtonCtx {
            has_files: self.list_len() > 0,
            group_index: self.current_group,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.inner
            .groups
            .get(self.current_group)
            .and_then(|g| g.files.get(cursor))
            .map(|f| f.path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 3,
        }
    }

    fn list_title(&self) -> String {
        let file_count = self.list_len();
        let group_num = self.current_group + 1;
        let total = self.inner.groups.len();
        format!(" Files ({}) [group {}/{}] ", file_count, group_num, total)
    }

    fn empty_message(&self) -> &'static str {
        "No disc extraction groups"
    }
}

impl GroupNavigation for DiscExtractionData {
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

/// Concrete resolution state for disc extraction modals.
pub type DiscExtractionState = ResolutionState<DiscExtractionData, DiscExtractionButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DiscExtractionAction {
    Apply,
    Skip,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DiscExtractionButton {
    Apply,
    Skip,
    #[default]
    Cancel,
}

pub struct DiscExtractionButtonCtx {
    pub has_files: bool,
    pub group_index: usize,
}

impl ModalButtons for DiscExtractionButton {
    type Context = DiscExtractionButtonCtx;
    type Action = DiscExtractionAction;

    fn all() -> &'static [Self] {
        &[Self::Apply, Self::Skip, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Apply => "Apply".into(),
            Self::Skip => "Skip".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Apply if ctx.has_files => Color::Cyan,
            Self::Apply => Color::DarkGray,
            Self::Skip if ctx.has_files => Color::Yellow,
            Self::Skip => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Apply | Self::Skip => ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> DiscExtractionAction {
        match self {
            Self::Apply => DiscExtractionAction::Apply,
            Self::Skip => DiscExtractionAction::Skip,
            Self::Cancel => DiscExtractionAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Apply => ProtocolBinding::Transaction {
                decision_key: DecisionKey::DiscExtraction {
                    group_index: ctx.group_index,
                },
                label: "Apply disc extraction".into(),
            },
            Self::Skip => ProtocolBinding::Navigation,
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::domain_queries::{DiscExtractionGroup, DiscFileEntry};

    fn make_file_entry(path: &str) -> DiscFileEntry {
        DiscFileEntry {
            inode: 1,
            path: path.to_string(),
            original_value: "Album, Disc 1".to_string(),
            cleaned_value: "Album".to_string(),
            source_tag: "ALBUM".to_string(),
        }
    }

    fn make_group(desc: &str, num_files: usize) -> DiscExtractionGroup {
        let files = (0..num_files)
            .map(|i| make_file_entry(&format!("corpus/{desc}/track_{i}.flac")))
            .collect();
        DiscExtractionGroup {
            description: desc.to_string(),
            disc_value: "1".to_string(),
            files,
        }
    }

    fn make_disc_data(num_groups: usize, files_per_group: usize) -> DiscExtractionModalData {
        let groups = (0..num_groups)
            .map(|i| make_group(&format!("disc_{i}"), files_per_group))
            .collect();
        DiscExtractionModalData { groups }
    }

    // -- GroupNavigation tests --

    #[test]
    fn zero_groups_navigation() {
        let data = DiscExtractionData::new(make_disc_data(0, 0));
        assert_eq!(data.group_count(), 0);
        assert!(!data.has_next());
        assert!(!data.has_prev());
    }

    #[test]
    fn three_groups_navigation() {
        let mut data = DiscExtractionData::new(make_disc_data(3, 2));
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
    fn list_len_returns_file_count() {
        let mut inner = DiscExtractionModalData::default();
        inner.groups.push(make_group("a", 3));
        inner.groups.push(make_group("b", 7));
        let data = DiscExtractionData::new(inner);
        assert_eq!(data.list_len(), 3);
    }

    #[test]
    fn list_len_zero_when_empty() {
        let data = DiscExtractionData::new(make_disc_data(0, 0));
        assert_eq!(data.list_len(), 0);
    }

    #[test]
    fn list_len_changes_with_current_group() {
        let mut inner = DiscExtractionModalData::default();
        inner.groups.push(make_group("small", 2));
        inner.groups.push(make_group("big", 6));
        let mut data = DiscExtractionData::new(inner);
        assert_eq!(data.list_len(), 2);
        data.current_group = 1;
        assert_eq!(data.list_len(), 6);
    }

    #[test]
    fn selected_path_valid_cursor() {
        let data = DiscExtractionData::new(make_disc_data(1, 3));
        assert_eq!(
            data.selected_path(0),
            Some("corpus/disc_0/track_0.flac")
        );
        assert_eq!(
            data.selected_path(2),
            Some("corpus/disc_0/track_2.flac")
        );
    }

    #[test]
    fn selected_path_out_of_bounds() {
        let data = DiscExtractionData::new(make_disc_data(1, 1));
        assert!(data.selected_path(99).is_none());
    }

    #[test]
    fn list_title_includes_group_position() {
        let data = DiscExtractionData::new(make_disc_data(3, 4));
        let title = data.list_title();
        assert!(title.contains("1/3"), "expected '1/3' in: {title}");
        assert!(title.contains("4"), "expected file count '4' in: {title}");
    }

    // -- ModalButtons tests --

    #[test]
    fn apply_and_skip_enabled_when_has_files() {
        let ctx = DiscExtractionButtonCtx {
            has_files: true,
            group_index: 0,
        };
        assert!(DiscExtractionButton::Apply.enabled(&ctx));
        assert!(DiscExtractionButton::Skip.enabled(&ctx));
    }

    #[test]
    fn apply_and_skip_disabled_when_empty() {
        let ctx = DiscExtractionButtonCtx {
            has_files: false,
            group_index: 0,
        };
        assert!(!DiscExtractionButton::Apply.enabled(&ctx));
        assert!(!DiscExtractionButton::Skip.enabled(&ctx));
    }

    #[test]
    fn cancel_always_enabled() {
        for has_files in [true, false] {
            let ctx = DiscExtractionButtonCtx {
                has_files,
                group_index: 0,
            };
            assert!(DiscExtractionButton::Cancel.enabled(&ctx));
        }
    }

    #[test]
    fn actions_return_correct_variants() {
        let ctx = DiscExtractionButtonCtx {
            has_files: true,
            group_index: 0,
        };
        assert_eq!(
            DiscExtractionButton::Apply.action(&ctx),
            DiscExtractionAction::Apply
        );
        assert_eq!(
            DiscExtractionButton::Skip.action(&ctx),
            DiscExtractionAction::Skip
        );
        assert_eq!(
            DiscExtractionButton::Cancel.action(&ctx),
            DiscExtractionAction::Cancel
        );
    }
}
