//! Missing File Resolution Modal Types
//!
//! Data structures for the missing file resolution modal, including
//! categorized files (restorable vs non-restorable) and button state.

pub use mm_meta::views::health_modals::*;

use mm_meta::paths::PathResolver;

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

/// Which action button is selected in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SelectedButton {
    RestoreAll,
    DropLost,
    #[default]
    Cancel,
}

impl SelectedButton {
    /// Move selection left.
    pub fn left(&mut self, has_restorable: bool) {
        *self = match *self {
            Self::Cancel => Self::DropLost,
            Self::DropLost => {
                if has_restorable {
                    Self::RestoreAll
                } else {
                    Self::DropLost
                }
            }
            Self::RestoreAll => Self::RestoreAll,
        };
    }

    /// Move selection right.
    pub fn right(&mut self, has_restorable: bool) {
        *self = match *self {
            Self::RestoreAll => Self::DropLost,
            Self::DropLost => Self::Cancel,
            Self::Cancel => Self::Cancel,
        };
        // Ensure we don't land on disabled buttons
        if *self == Self::RestoreAll && !has_restorable {
            self.right(has_restorable);
        }
    }
}
