//! Subpar Duplicate resolution — stash subpar duplicates and drop from index.
//!
//! Route: `/resolve/subpar-duplicates`
//! Query: `GetSubparDuplicateData`
//! Data: `SubparDuplicateModalData` (mm-meta)
//! Mutations: `StashFromZone` + `DropFromIndex` per file

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::health_modals::SubparDuplicateModalData;

use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData`.
pub struct SubparDuplicateData(pub SubparDuplicateModalData);

impl ResolutionData for SubparDuplicateData {
    type ButtonCtx = SubparDuplicateModalData;

    fn list_len(&self) -> usize {
        self.0.files.len()
    }

    fn button_ctx(&self) -> SubparDuplicateModalData {
        self.0.clone()
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.0.files.get(cursor).map(|f| f.corpus_path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        // Dynamic detail height from path lengths (approximate)
        ContentLayout::ListAboveDetail { detail_height: 6 }
    }

    fn list_title(&self) -> String {
        format!(" Subpar Files ({}) ", self.0.files.len())
    }

    fn empty_message(&self) -> &'static str {
        "No subpar duplicates found"
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for subpar duplicate modals.
pub type SubparDuplicateState = ResolutionState<SubparDuplicateData, SubparButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubparDuplicateAction {
    /// User confirmed stash + drop action.
    ConfirmStashAll,
    /// Cancel and return to Insights view.
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for SubparDuplicateState {
    type Action = SubparDuplicateAction;

    fn dispatch(
        &self,
        action: SubparDuplicateAction,
        resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            SubparDuplicateAction::ConfirmStashAll => {
                let mutations = self.data.0.stash_and_drop_mutations(resolver);
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = self.data.button_ctx();
                let key = SubparButton::StashAll
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: "Stash subpar duplicates".into(),
                    mutations,
                }
            }
            SubparDuplicateAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Subpar duplicate resolution cancelled"
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubparButton {
    StashAll,
    #[default]
    Cancel,
}

impl ModalButtons for SubparButton {
    type Context = SubparDuplicateModalData;
    type Action = SubparDuplicateAction;

    fn all() -> &'static [Self] {
        &[Self::StashAll, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::StashAll => "Stash & Drop All".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::StashAll if ctx.has_files() => Color::Cyan,
            Self::StashAll => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::StashAll => ctx.has_files(),
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> SubparDuplicateAction {
        match self {
            Self::StashAll => SubparDuplicateAction::ConfirmStashAll,
            Self::Cancel => SubparDuplicateAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::StashAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::SubparDuplicate,
                label: "Stash subpar duplicates".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
