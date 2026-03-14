//! Types for moved file acknowledgement modal.

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::views::MovedFileInfo;
use mm_ui::modal_buttons::ModalButtons;
use mm_ui::modal_frame::ContentLayout;
use mm_ui::protocol_binding::ProtocolBinding;
use mm_ui::resolution_state::{ResolutionData, ResolutionState};

// ============================================================================
// Data wrapper + ResolutionData impl
// ============================================================================

/// Data payload for the moved file acknowledgement modal.
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
// Concrete state type alias
// ============================================================================

/// State for the moved file acknowledgement modal.
pub type MovedFileState = ResolutionState<MovedFileData, MovedFileButton>;

// ============================================================================
// Button Definition
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

// ============================================================================
// Action Enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MovedFileAction {
    /// Acknowledge all moved files (updates paths in DB, clears signals)
    Acknowledge,
    /// Cancel and return to Insights
    Cancel,
}
