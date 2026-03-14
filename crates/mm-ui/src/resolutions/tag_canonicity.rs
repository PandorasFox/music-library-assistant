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
// Data wrapper
// ============================================================================

/// Wraps the mm-meta packed type to implement `ResolutionData` + `GroupNavigation`.
pub struct TagCanonicityData {
    pub inner: TagCanonicityResolutionData,
    pub current_cluster: usize,
}

impl TagCanonicityData {
    pub fn new(data: TagCanonicityResolutionData) -> Self {
        Self {
            inner: data,
            current_cluster: 0,
        }
    }
}

impl ResolutionData for TagCanonicityData {
    type ButtonCtx = CanonicityButtonCtx;

    fn list_len(&self) -> usize {
        self.inner
            .clusters
            .get(self.current_cluster)
            .map_or(0, |c| c.outlier_variants.len())
    }

    fn button_ctx(&self) -> CanonicityButtonCtx {
        CanonicityButtonCtx {
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
    pub has_outliers: bool,
    pub current_cluster_index: usize,
}

impl ModalButtons for CanonicityButton {
    type Context = CanonicityButtonCtx;
    type Action = CanonicityAction;

    fn all() -> &'static [Self] {
        &[Self::Confirm, Self::FlagCanonical, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Confirm => "Confirm".into(),
            Self::FlagCanonical => "Flag Canonical".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Confirm if ctx.has_outliers => Color::Green,
            Self::Confirm => Color::DarkGray,
            Self::FlagCanonical => Color::Yellow,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Confirm => ctx.has_outliers,
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
                    tag_name: String::new(),
                    cluster_index: ctx.current_cluster_index,
                },
                label: "Confirm tag canonicity".into(),
            },
            Self::FlagCanonical => ProtocolBinding::Transaction {
                decision_key: DecisionKey::TagCanonicity {
                    tag_name: String::new(),
                    cluster_index: ctx.current_cluster_index,
                },
                label: "Flag as canonical".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
