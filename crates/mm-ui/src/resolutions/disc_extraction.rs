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
