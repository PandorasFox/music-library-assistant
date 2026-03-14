//! Corrupt File Resolution Modal Types

pub use mm_meta::views::health_modals::{CorruptFileEntry, CorruptFileModalData};

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
// Data wrapper + ResolutionData impl
// ============================================================================

/// Data payload for the corrupt file resolution modal.
pub struct CorruptFileData(pub CorruptFileModalData);

impl ResolutionData for CorruptFileData {
    type ButtonCtx = CorruptButtonCtx;

    fn list_len(&self) -> usize {
        self.0.files.len()
    }

    fn button_ctx(&self) -> CorruptButtonCtx {
        CorruptButtonCtx {
            has_files: self.0.has_files(),
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.0.files.get(cursor).map(|f| f.corpus_path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 3,
        }
    }

    fn list_title(&self) -> String {
        format!(" Corrupt Files ({}) ", self.0.files.len())
    }

    fn empty_message(&self) -> &'static str {
        "No corrupt files found"
    }
}

// ============================================================================
// Concrete state type alias
// ============================================================================

/// State for the corrupt file resolution modal.
pub type CorruptFilePreviewState = ResolutionState<CorruptFileData, CorruptButton>;

// ============================================================================
// Action Enum
// ============================================================================

/// Actions returned from the corrupt file preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorruptFilePreviewAction {
    /// User confirmed stash + drop action.
    ConfirmStashAll,
    /// Cancel and return to Insights view.
    Cancel,
}

// ============================================================================
// Mutation builder
// ============================================================================

/// Generate StashFromZone + DropFromIndex mutations for all corrupt files.
pub fn stash_and_drop_mutations(
    data: &CorruptFileModalData,
    resolver: &mm_meta::paths::PathResolver,
) -> Vec<Mutation> {
    data.files
        .iter()
        .flat_map(|f| stash_file_mutations(&f.corpus_path, f.inode, "corrupt", resolver))
        .collect()
}

// ============================================================================
// Button Definition
// ============================================================================

/// Lightweight context for button enablement/labels.
pub struct CorruptButtonCtx {
    pub has_files: bool,
}

/// Button choices for the corrupt file resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CorruptButton {
    StashAll,
    #[default]
    Cancel,
}

impl ModalButtons for CorruptButton {
    type Context = CorruptButtonCtx;
    type Action = CorruptFilePreviewAction;

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
            Self::StashAll if ctx.has_files => Color::Red,
            Self::StashAll => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::StashAll => ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> CorruptFilePreviewAction {
        match self {
            Self::StashAll => CorruptFilePreviewAction::ConfirmStashAll,
            Self::Cancel => CorruptFilePreviewAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::StashAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::CorruptFile,
                label: "Stash corrupt files".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
