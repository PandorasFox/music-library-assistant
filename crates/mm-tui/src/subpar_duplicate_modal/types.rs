//! Subpar Duplicate Resolution Modal Types
//!
//! Data structures for the subpar duplicate resolution modal, including
//! file entries and button state.

pub use mm_meta::views::health_modals::{SubparDuplicateModalData, SubparFileEntry};

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::decisions::DecisionKey;
use mm_meta::mutations::Mutation;
use mm_ui::protocol_binding::ProtocolBinding;
use crate::helpers::stash_file_mutations;
use crate::widgets::modal_buttons::ModalButtons;

use super::preview::SubparDuplicatePreviewAction;

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

/// Button choices for the subpar duplicate resolution modal.
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
