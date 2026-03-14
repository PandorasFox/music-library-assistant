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
