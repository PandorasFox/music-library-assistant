//! Missing Directory resolution — directories deleted externally, drop from index.
//!
//! Route: `/resolve/missing-directories`
//! Query: `GetMissingDirectoryData`
//! Data: `MissingDirectoryModalData` (mm-meta)
//! Mutations: `DropDirectoryFromIndex` per directory

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::health_modals::MissingDirectoryModalData;

use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps the mm-meta wire type to implement `ResolutionData`.
pub struct MissingDirectoryData(pub MissingDirectoryModalData);

impl ResolutionData for MissingDirectoryData {
    type ButtonCtx = MissingDirectoryButtonCtx;

    fn list_len(&self) -> usize {
        self.0.count()
    }

    fn button_ctx(&self) -> MissingDirectoryButtonCtx {
        MissingDirectoryButtonCtx {
            has_directories: self.0.count() > 0,
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.0.directories.get(cursor).map(|s| s.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 2,
        }
    }

    fn list_title(&self) -> String {
        format!(" Deleted Directories ({}) ", self.0.count())
    }

    fn empty_message(&self) -> &'static str {
        "No missing directories"
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for missing directory modals.
pub type MissingDirectoryState = ResolutionState<MissingDirectoryData, MissingDirectoryButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingDirectoryAction {
    ConfirmDrop,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingDirectoryButton {
    Drop,
    #[default]
    Cancel,
}

pub struct MissingDirectoryButtonCtx {
    pub has_directories: bool,
}

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for MissingDirectoryState {
    type Action = MissingDirectoryAction;

    fn dispatch(
        &self,
        action: MissingDirectoryAction,
        _resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            MissingDirectoryAction::ConfirmDrop => {
                let mutations = self.data.0.drop_mutations();
                if mutations.is_empty() {
                    return DispatchResult::Handled;
                }

                let ctx = self.data.button_ctx();
                let key = MissingDirectoryButton::Drop
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label: "Drop missing directories".into(),
                    mutations,
                }
            }
            MissingDirectoryAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Missing directory resolution cancelled"
    }
}

impl ModalButtons for MissingDirectoryButton {
    type Context = MissingDirectoryButtonCtx;
    type Action = MissingDirectoryAction;

    fn all() -> &'static [Self] {
        &[Self::Drop, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Drop => "Drop All".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::Drop if ctx.has_directories => Color::Yellow,
            Self::Drop => Color::DarkGray,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Drop => ctx.has_directories,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingDirectoryAction {
        match self {
            Self::Drop => MissingDirectoryAction::ConfirmDrop,
            Self::Cancel => MissingDirectoryAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Drop => ProtocolBinding::Transaction {
                decision_key: DecisionKey::MissingDirectory,
                label: "Drop missing directories".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
