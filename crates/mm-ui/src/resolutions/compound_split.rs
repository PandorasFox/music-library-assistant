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
