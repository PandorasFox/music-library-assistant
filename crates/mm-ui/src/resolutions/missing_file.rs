//! Missing File resolution — button and action types only.
//!
//! The missing file modal has a bespoke dual-pane state (restorable vs
//! non-restorable) that doesn't use the generic `ResolutionState`. Only
//! the button enum and action types are lifted here.
//!
//! Route: `/resolve/missing-files/restorable`, `/resolve/missing-files/permanent`
//! Query: `GetMissingFileData`

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;

use crate::modal_buttons::ModalButtons;
use crate::protocol_binding::ProtocolBinding;

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MissingFileAction {
    None,
    ConfirmRestore,
    ConfirmDrop,
    Cancel,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingFileButton {
    RestoreAll,
    DropLost,
    #[default]
    Cancel,
}

pub struct MissingFileButtonCtx {
    pub has_restorable: bool,
}

impl ModalButtons for MissingFileButton {
    type Context = MissingFileButtonCtx;
    type Action = MissingFileAction;

    fn all() -> &'static [Self] {
        &[Self::RestoreAll, Self::DropLost, Self::Cancel]
    }

    fn label(&self, _ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::RestoreAll => "Restore All".into(),
            Self::DropLost => "Drop Missing".into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, ctx: &Self::Context) -> Color {
        match self {
            Self::RestoreAll if ctx.has_restorable => Color::Green,
            Self::RestoreAll => Color::DarkGray,
            Self::DropLost => Color::Red,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::RestoreAll => ctx.has_restorable,
            Self::DropLost => true,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingFileAction {
        match self {
            Self::RestoreAll => MissingFileAction::ConfirmRestore,
            Self::DropLost => MissingFileAction::ConfirmDrop,
            Self::Cancel => MissingFileAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::RestoreAll => ProtocolBinding::Transaction {
                decision_key: DecisionKey::MissingFile,
                label: "Restore missing files".into(),
            },
            Self::DropLost => ProtocolBinding::Transaction {
                decision_key: DecisionKey::MissingFile,
                label: "Drop missing files".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
