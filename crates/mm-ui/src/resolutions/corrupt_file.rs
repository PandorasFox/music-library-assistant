//! Corrupt File resolution — stash corrupt files and drop from index.
//!
//! Route: `/resolve/corrupt-files`
//! Query: `GetCorruptFileData`
//! Data: `CorruptFileModalData` (mm-meta)
//! Mutations: `StashFromZone` + `DropFromIndex` per file

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::health_modals::CorruptFileModalData;

use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData`.
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
// State type alias
// ============================================================================

/// Concrete resolution state for corrupt file modals.
pub type CorruptFileState = ResolutionState<CorruptFileData, CorruptButton>;

// ============================================================================
// Action enum
// ============================================================================

/// Actions returned from the corrupt file resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CorruptFileAction {
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

impl super::dispatch::Dispatchable for CorruptFileState {
    type Action = CorruptFileAction;

    fn dispatch(
        &self,
        action: CorruptFileAction,
        resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            CorruptFileAction::ConfirmStashAll => {
                let mutations = self.data.0.stash_and_drop_mutations(resolver);
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = self.data.button_ctx();
                let key = CorruptButton::StashAll
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: "Stash corrupt files".into(),
                    mutations,
                }
            }
            CorruptFileAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Corrupt file resolution cancelled"
    }
}

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
    type Action = CorruptFileAction;

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

    fn action(&self, _ctx: &Self::Context) -> CorruptFileAction {
        match self {
            Self::StashAll => CorruptFileAction::ConfirmStashAll,
            Self::Cancel => CorruptFileAction::Cancel,
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
