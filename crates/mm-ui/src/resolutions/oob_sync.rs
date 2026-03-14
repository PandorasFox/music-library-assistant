//! OOB Tag Sync resolution — button types for out-of-band tag sync modal.
//!
//! Route: `/resolve/oob-sync`
//! Query: `GetOobSyncFiles`
//! Data: `OobSyncFile` (mm-meta)
//!
//! NOTE: Only button/action/context types live here. The full `OobSyncState`
//! remains in mm-tui because it uses complex state (bulk selection, filtering)
//! that doesn't fit the `ResolutionState` pattern yet.

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;

use crate::modal_buttons::ModalButtons;
use crate::protocol_binding::ProtocolBinding;

// ============================================================================
// Button context
// ============================================================================

/// Context for OobSyncButton enablement.
#[derive(Debug, Clone, Copy)]
pub struct OobSyncButtonCtx {
    pub disk_to_index_count: usize,
    pub index_to_disk_count: usize,
}

// ============================================================================
// Button enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OobSyncButton {
    #[default]
    AcceptDisk,
    AcceptDb,
    Cancel,
}

impl ModalButtons for OobSyncButton {
    type Context = OobSyncButtonCtx;
    type Action = OobSyncAction;

    fn all() -> &'static [Self] {
        &[Self::AcceptDisk, Self::AcceptDb, Self::Cancel]
    }

    fn label(&self, ctx: &Self::Context) -> Cow<'static, str> {
        match self {
            Self::AcceptDisk => format!("Accept Disk ({})", ctx.disk_to_index_count).into(),
            Self::AcceptDb => format!("Accept DB ({})", ctx.index_to_disk_count).into(),
            Self::Cancel => "Cancel".into(),
        }
    }

    fn color(&self, _ctx: &Self::Context) -> Color {
        match self {
            Self::AcceptDisk => Color::Cyan,
            Self::AcceptDb => Color::Magenta,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::AcceptDisk => ctx.disk_to_index_count > 0,
            Self::AcceptDb => ctx.index_to_disk_count > 0,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> OobSyncAction {
        match self {
            Self::AcceptDisk => OobSyncAction::AcceptDisk,
            Self::AcceptDb => OobSyncAction::AcceptDb,
            Self::Cancel => OobSyncAction::Cancel,
        }
    }

    fn protocol_binding(&self, _ctx: &Self::Context) -> ProtocolBinding {
        match self {
            Self::AcceptDisk => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobSync,
                label: "Sync disk tags \u{2192} index".into(),
            },
            Self::AcceptDb => ProtocolBinding::Transaction {
                decision_key: DecisionKey::OobSync,
                label: "Sync index tags \u{2192} disk".into(),
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}

// ============================================================================
// Action enum
// ============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OobSyncAction {
    None,
    /// Accept disk values — sync DiskToIndex files to DB
    AcceptDisk,
    /// Accept DB values — sync IndexToDisk files to disk
    AcceptDb,
    /// Cancel and return to Insights
    Cancel,
}
