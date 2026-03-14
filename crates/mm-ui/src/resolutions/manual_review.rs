//! Manual Review resolution — shared types for all four group-review modals.
//!
//! Routes:
//!   `/resolve/redundant-duplicates`
//!   `/resolve/deploy-conflicts`
//!   `/resolve/metadata-duplicates`
//!   `/resolve/same-recording`
//!
//! All share the same data shape (`ManualReviewData`) and similar buttons,
//! differing only by `ReviewKind` (which gates `MarkExpected` enablement).

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::review_match::{ManualReviewData, ReviewKind};

use crate::group_navigation::GroupNavigation;
use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps `ManualReviewData` + navigation state for `ResolutionData` + `GroupNavigation`.
pub struct ManualReviewResolutionData {
    pub inner: ManualReviewData,
    pub current_group: usize,
    pub review_kind: ReviewKind,
}

impl ManualReviewResolutionData {
    pub fn new(data: ManualReviewData, review_kind: ReviewKind) -> Self {
        Self {
            inner: data,
            current_group: 0,
            review_kind,
        }
    }
}

impl ResolutionData for ManualReviewResolutionData {
    type ButtonCtx = ReviewButtonCtx;

    fn list_len(&self) -> usize {
        self.inner
            .groups
            .get(self.current_group)
            .map_or(0, |g| g.files.len())
    }

    fn button_ctx(&self) -> ReviewButtonCtx {
        ReviewButtonCtx {
            has_files: self.list_len() > 0,
            review_kind: self.review_kind,
            current_group_index: self.current_group,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.inner
            .groups
            .get(self.current_group)
            .and_then(|g| g.files.get(cursor))
            .map(|f| f.corpus_path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::HorizontalSplit {
            list_percent: 50,
            info_height: 3,
        }
    }

    fn list_title(&self) -> String {
        let group_num = self.current_group + 1;
        let total = self.inner.groups.len();
        let label = self
            .inner
            .groups
            .get(self.current_group)
            .map(|g| g.label.as_str())
            .unwrap_or("???");
        format!(" {} ({}/{}) ", label, group_num, total)
    }

    fn empty_message(&self) -> &'static str {
        "No review groups"
    }
}

impl GroupNavigation for ManualReviewResolutionData {
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

/// Concrete resolution state for manual review modals.
pub type ManualReviewState = ResolutionState<ManualReviewResolutionData, ReviewButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewAction {
    Stash,
    MarkExpected,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ReviewButton {
    Stash,
    MarkExpected,
    #[default]
    Cancel,
}

pub struct ReviewButtonCtx {
    pub has_files: bool,
    pub review_kind: ReviewKind,
    pub current_group_index: usize,
}

impl ModalButtons for ReviewButton {
    type Context = ReviewButtonCtx;
    type Action = ReviewAction;

    fn all() -> &'static [Self] {
        &[Self::Stash, Self::MarkExpected, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Stash => "Stash Selected".into(),
            Self::MarkExpected => "Mark Expected".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Stash if ctx.has_files => Color::Red,
            Self::Stash => Color::DarkGray,
            Self::MarkExpected
                if ctx.review_kind == ReviewKind::RedundantDuplicate =>
            {
                Color::Yellow
            }
            Self::MarkExpected => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Stash => ctx.has_files,
            Self::MarkExpected => {
                ctx.review_kind == ReviewKind::RedundantDuplicate
            }
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> ReviewAction {
        match self {
            Self::Stash => ReviewAction::Stash,
            Self::MarkExpected => ReviewAction::MarkExpected,
            Self::Cancel => ReviewAction::Cancel,
        }
    }

    fn protocol_binding(&self, ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Stash => ProtocolBinding::Transaction {
                decision_key: DecisionKey::ManualReview {
                    group_index: ctx.current_group_index,
                },
                label: ctx.review_kind.transaction_label().into(),
            },
            Self::MarkExpected => ProtocolBinding::Transaction {
                decision_key: DecisionKey::ManualReview {
                    group_index: ctx.current_group_index,
                },
                label: "Mark expected duplicate".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mm_meta::views::review_match::{FileMetaSummary, ReviewFileEntry, ReviewGroup};

    fn make_file_entry(path: &str) -> ReviewFileEntry {
        ReviewFileEntry {
            corpus_path: path.to_string(),
            inode: 1,
            context: String::new(),
            stashed: false,
            meta: Some(FileMetaSummary::default()),
        }
    }

    fn make_group(label: &str, num_files: usize) -> ReviewGroup {
        let files = (0..num_files)
            .map(|i| make_file_entry(&format!("corpus/{label}/track_{i}.flac")))
            .collect();
        ReviewGroup {
            label: label.to_string(),
            files,
            signal_key: None,
        }
    }

    fn make_review_data(num_groups: usize, files_per_group: usize) -> ManualReviewData {
        let groups = (0..num_groups)
            .map(|i| make_group(&format!("group_{i}"), files_per_group))
            .collect();
        ManualReviewData { groups }
    }

    // -- GroupNavigation tests --

    #[test]
    fn zero_groups_navigation() {
        let data = ManualReviewResolutionData::new(
            make_review_data(0, 0),
            ReviewKind::RedundantDuplicate,
        );
        assert_eq!(data.group_count(), 0);
        assert!(!data.has_next());
        assert!(!data.has_prev());
    }

    #[test]
    fn three_groups_navigation() {
        let mut data = ManualReviewResolutionData::new(
            make_review_data(3, 2),
            ReviewKind::RedundantDuplicate,
        );
        // At group 0
        assert_eq!(data.group_count(), 3);
        assert!(data.has_next());
        assert!(!data.has_prev());

        // At group 1
        data.current_group = 1;
        assert!(data.has_next());
        assert!(data.has_prev());

        // At group 2 (last)
        data.current_group = 2;
        assert!(!data.has_next());
        assert!(data.has_prev());
    }

    // -- ResolutionData tests --

    #[test]
    fn list_len_returns_file_count_for_current_group() {
        let mut inner = make_review_data(0, 0);
        inner.groups.push(make_group("small", 2));
        inner.groups.push(make_group("big", 5));
        let data = ManualReviewResolutionData::new(inner, ReviewKind::DeployConflict);
        assert_eq!(data.list_len(), 2);
    }

    #[test]
    fn list_len_zero_when_groups_empty() {
        let data = ManualReviewResolutionData::new(
            make_review_data(0, 0),
            ReviewKind::DeployConflict,
        );
        assert_eq!(data.list_len(), 0);
    }

    #[test]
    fn list_len_changes_with_current_group() {
        let mut inner = ManualReviewData::default();
        inner.groups.push(make_group("a", 1));
        inner.groups.push(make_group("b", 4));
        let mut data =
            ManualReviewResolutionData::new(inner, ReviewKind::MetadataDuplicate);
        assert_eq!(data.list_len(), 1);
        data.current_group = 1;
        assert_eq!(data.list_len(), 4);
    }

    #[test]
    fn selected_path_returns_corpus_path() {
        let data = ManualReviewResolutionData::new(
            make_review_data(1, 3),
            ReviewKind::RedundantDuplicate,
        );
        assert_eq!(
            data.selected_path(0),
            Some("corpus/group_0/track_0.flac")
        );
        assert_eq!(
            data.selected_path(2),
            Some("corpus/group_0/track_2.flac")
        );
    }

    #[test]
    fn selected_path_out_of_bounds_returns_none() {
        let data = ManualReviewResolutionData::new(
            make_review_data(1, 2),
            ReviewKind::RedundantDuplicate,
        );
        assert!(data.selected_path(99).is_none());
    }

    #[test]
    fn list_title_includes_group_position() {
        let mut data = ManualReviewResolutionData::new(
            make_review_data(3, 1),
            ReviewKind::RedundantDuplicate,
        );
        let title = data.list_title();
        assert!(title.contains("1/3"), "expected '1/3' in: {title}");

        data.current_group = 2;
        let title = data.list_title();
        assert!(title.contains("3/3"), "expected '3/3' in: {title}");
    }

    // -- ModalButtons tests: ReviewKind-specific enablement --

    #[test]
    fn mark_expected_enabled_only_for_redundant_duplicate() {
        let kinds = [
            (ReviewKind::RedundantDuplicate, true),
            (ReviewKind::DeployConflict, false),
            (ReviewKind::MetadataDuplicate, false),
            (ReviewKind::SameRecordingDifferentRelease, false),
        ];
        for (kind, expected_enabled) in kinds {
            let ctx = ReviewButtonCtx {
                has_files: true,
                review_kind: kind,
                current_group_index: 0,
            };
            assert_eq!(
                ReviewButton::MarkExpected.enabled(&ctx),
                expected_enabled,
                "MarkExpected enablement wrong for {:?}",
                kind,
            );
        }
    }

    #[test]
    fn stash_enabled_when_has_files_disabled_when_empty() {
        let ctx_with = ReviewButtonCtx {
            has_files: true,
            review_kind: ReviewKind::RedundantDuplicate,
            current_group_index: 0,
        };
        let ctx_without = ReviewButtonCtx {
            has_files: false,
            review_kind: ReviewKind::RedundantDuplicate,
            current_group_index: 0,
        };
        assert!(ReviewButton::Stash.enabled(&ctx_with));
        assert!(!ReviewButton::Stash.enabled(&ctx_without));
    }

    #[test]
    fn cancel_always_enabled() {
        for has_files in [true, false] {
            let ctx = ReviewButtonCtx {
                has_files,
                review_kind: ReviewKind::DeployConflict,
                current_group_index: 0,
            };
            assert!(ReviewButton::Cancel.enabled(&ctx));
        }
    }

    #[test]
    fn actions_return_correct_variants() {
        let ctx = ReviewButtonCtx {
            has_files: true,
            review_kind: ReviewKind::RedundantDuplicate,
            current_group_index: 0,
        };
        assert_eq!(ReviewButton::Stash.action(&ctx), ReviewAction::Stash);
        assert_eq!(
            ReviewButton::MarkExpected.action(&ctx),
            ReviewAction::MarkExpected
        );
        assert_eq!(ReviewButton::Cancel.action(&ctx), ReviewAction::Cancel);
    }

    #[test]
    fn button_ctx_reflects_data_state() {
        let data = ManualReviewResolutionData::new(
            make_review_data(1, 3),
            ReviewKind::DeployConflict,
        );
        let ctx = data.button_ctx();
        assert!(ctx.has_files);
        assert_eq!(ctx.review_kind, ReviewKind::DeployConflict);
        assert_eq!(ctx.current_group_index, 0);

        let empty = ManualReviewResolutionData::new(
            make_review_data(0, 0),
            ReviewKind::MetadataDuplicate,
        );
        let ctx = empty.button_ctx();
        assert!(!ctx.has_files);
    }
}
