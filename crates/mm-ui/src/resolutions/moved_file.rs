//! Moved File resolution — acknowledge files relocated externally.
//!
//! Route: `/resolve/moved-files`
//! Query: `GetMovedFiles`
//! Data: `MovedFileInfo` (mm-meta, wrapped in `MovedFileData`)
//! Mutations: `UpdateFilePath` per file

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::MovedFileInfo;

use crate::modal_buttons::ModalButtons;
use crate::modal_frame::ContentLayout;
use crate::protocol_binding::ProtocolBinding;
use crate::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper
// ============================================================================

/// Wraps a `Vec<MovedFileInfo>` to implement `ResolutionData`.
pub struct MovedFileData {
    pub files: Vec<MovedFileInfo>,
}

impl ResolutionData for MovedFileData {
    type ButtonCtx = MovedFileButtonCtx;

    fn list_len(&self) -> usize {
        self.files.len()
    }

    fn button_ctx(&self) -> MovedFileButtonCtx {
        MovedFileButtonCtx {
            has_files: !self.files.is_empty(),
        }
    }

    fn selected_path(&self, cursor: usize) -> Option<&str> {
        self.files.get(cursor).map(|f| f.new_path.as_str())
    }

    fn content_layout(&self) -> ContentLayout {
        ContentLayout::FourSection {
            header_height: 3,
            detail_height: 6,
        }
    }

    fn list_title(&self) -> String {
        "Files".to_string()
    }

    fn empty_message(&self) -> &'static str {
        "No moved files to acknowledge"
    }
}

// ============================================================================
// State type alias
// ============================================================================

/// Concrete resolution state for moved file acknowledgement modals.
pub type MovedFileState = ResolutionState<MovedFileData, MovedFileButton>;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovedFileAction {
    /// Acknowledge all moved files (updates paths in DB, clears signals)
    Acknowledge,
    /// Cancel and return to Insights
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MovedFileButton {
    Acknowledge,
    #[default]
    Cancel,
}

/// Context for button enablement/labels.
pub struct MovedFileButtonCtx {
    pub has_files: bool,
}

// ============================================================================
// Dispatchable
// ============================================================================

impl super::dispatch::Dispatchable for MovedFileState {
    type Action = MovedFileAction;

    fn dispatch(
        &self,
        action: MovedFileAction,
        _resolver: &mm_meta::paths::PathResolver,
    ) -> super::dispatch::DispatchResult {
        use super::dispatch::DispatchResult;

        match action {
            MovedFileAction::Acknowledge => {
                if self.data.files.is_empty() {
                    return DispatchResult::Handled;
                }

                let mutations: Vec<_> = self.data.files.iter().map(|f| f.to_update_mutation()).collect();
                let label = format!(
                    "Acknowledge {} moved file{}",
                    mutations.len(),
                    if mutations.len() == 1 { "" } else { "s" }
                );

                let ctx = self.data.button_ctx();
                let key = MovedFileButton::Acknowledge
                    .protocol_binding(&ctx)
                    .decision_key()
                    .unwrap()
                    .clone();

                DispatchResult::Stage {
                    key,
                    label,
                    mutations,
                }
            }
            MovedFileAction::Cancel => DispatchResult::Cancel,
        }
    }

    fn cancel_message(&self) -> &'static str {
        "Moved file acknowledgement cancelled"
    }
}

impl ModalButtons for MovedFileButton {
    type Context = MovedFileButtonCtx;
    type Action = MovedFileAction;

    fn all() -> &'static [Self] {
        &[Self::Acknowledge, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::Acknowledge => "Acknowledge".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::Acknowledge => Color::Green,
            Self::Cancel => Color::Red,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::Acknowledge => ctx.has_files,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MovedFileAction {
        match self {
            Self::Acknowledge => MovedFileAction::Acknowledge,
            Self::Cancel => MovedFileAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::Acknowledge => ProtocolBinding::Transaction {
                decision_key: DecisionKey::MovedFile,
                label: "Acknowledge moved files".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
