//! Subpar Duplicate Resolution Modal Types

pub use mm_meta::views::health_modals::{SubparDuplicateModalData, SubparFileEntry};

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::Mutation;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::modal_frame::ContentLayout;
use mm_ui::protocol_binding::ProtocolBinding;
use mm_ui::resolution_state::{ResolutionData, ResolutionState};

use crate::helpers::stash_file_mutations;

// ============================================================================
// Data wrapper
// ============================================================================

/// Data payload for the subpar duplicate resolution modal.
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
// Concrete state type alias
// ============================================================================

pub type SubparDuplicatePreviewState = ResolutionState<SubparDuplicateData, SubparButton>;

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubparDuplicatePreviewAction {
    /// User confirmed stash + drop action.
    ConfirmStashAll,
    /// Cancel and return to Insights view.
    Cancel,
}

// ============================================================================
// Mutation builder
// ============================================================================

/// Generate StashFromZone + DropFromIndex mutations for all subpar files.
pub fn stash_and_drop_mutations(
    data: &SubparDuplicateModalData,
    resolver: &mm_meta::paths::PathResolver,
) -> Vec<Mutation> {
    data.files
        .iter()
        .flat_map(|f| stash_file_mutations(&f.corpus_path, f.inode, "subpar", resolver))
        .collect()
}

// ============================================================================
// Button Definition
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SubparButton {
    StashAll,
    #[default]
    Cancel,
}

impl ModalButtons for SubparButton {
    type Context = SubparDuplicateModalData;
    type Action = SubparDuplicatePreviewAction;

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

    fn action(&self, _ctx: &Self::Context) -> SubparDuplicatePreviewAction {
        match self {
            Self::StashAll => SubparDuplicatePreviewAction::ConfirmStashAll,
            Self::Cancel => SubparDuplicatePreviewAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::StashAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::SubparDuplicate,
                label: "Stash subpar duplicates".into(),
                data_query: None,
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
