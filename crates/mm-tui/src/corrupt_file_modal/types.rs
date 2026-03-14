//! Corrupt File Resolution Modal Types
//!
//! Data structures for the corrupt file resolution modal, including
//! file entries and button state.

pub use mm_meta::views::health_modals::{CorruptFileEntry, CorruptFileModalData};

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::Mutation;
use mm_ui::protocol_binding::ProtocolBinding;
use crate::helpers::stash_file_mutations;
use crate::widgets::modal_buttons::ModalButtons;

use super::preview::CorruptFilePreviewAction;

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
                data_query: None,
            },
            Self::Cancel => ProtocolBinding::Navigation,
        }
    }
}
