//! Missing File Resolution Modal Types
//!
//! Data structures for the missing file resolution modal, including
//! categorized files (restorable vs non-restorable) and button state.

pub use mm_meta::views::health_modals::*;

use std::borrow::Cow;

use ratatui::style::Color;

use mm_meta::paths::PathResolver;
use crate::widgets::modal_buttons::ModalButtons;

use super::preview::MissingFilePreviewAction;

/// Generate HardLink mutations for restorable files.
///
/// Paths stored in RestorableMissingFile are relative to their roots:
///   - library_path: relative to libraries_root
///   - corpus_path: relative to corpus_root
///
/// These must be resolved to absolute for HardLink filesystem operations.
pub fn restore_mutations(data: &MissingFileModalData, resolver: &PathResolver) -> Vec<mm_meta::mutations::Mutation> {
    data.restorable
        .iter()
        .map(|f| {
            // Resolve relative paths to absolute
            let source = resolver.resolve(std::path::Path::new(&f.library_path));
            let destination = resolver.resolve(std::path::Path::new(&f.corpus_path));
            mm_meta::mutations::Mutation::HardLink(
                mm_meta::mutations::file_ops::HardLinkMutation {
                    source,
                    destination,
                },
            )
        })
        .collect()
}

/// Button choices for the missing file resolution modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MissingFileButton {
    RestoreAll,
    DropLost,
    #[default]
    Cancel,
}

impl ModalButtons for MissingFileButton {
    type Context = MissingFileModalData;
    type Action = MissingFilePreviewAction;

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
            Self::RestoreAll if ctx.has_restorable() => Color::Green,
            Self::RestoreAll => Color::DarkGray,
            Self::DropLost => Color::Red,
            Self::Cancel => Color::White,
        }
    }

    fn enabled(&self, ctx: &Self::Context) -> bool {
        match self {
            Self::RestoreAll => ctx.has_restorable(),
            Self::DropLost => true,
            Self::Cancel => true,
        }
    }

    fn action(&self, _ctx: &Self::Context) -> MissingFilePreviewAction {
        match self {
            Self::RestoreAll => MissingFilePreviewAction::ConfirmRestore,
            Self::DropLost => MissingFilePreviewAction::ConfirmDrop,
            Self::Cancel => MissingFilePreviewAction::Cancel,
        }
    }
}
