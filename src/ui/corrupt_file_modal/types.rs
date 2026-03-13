//! Corrupt File Resolution Modal Types
//!
//! Data structures for the corrupt file resolution modal, including
//! file entries and button state.

pub use mm_meta::views::health_modals::{CorruptFileEntry, CorruptFileModalData};

use crate::meta::mutations::Mutation;
use crate::ui::helpers::stash_file_mutations;

/// Generate StashFromZone + DropFromIndex mutations for all corrupt files.
pub fn stash_and_drop_mutations(
    data: &CorruptFileModalData,
    resolver: &crate::corpus::paths::PathResolver,
) -> Vec<Mutation> {
    data.files
        .iter()
        .flat_map(|f| stash_file_mutations(&f.corpus_path, f.inode, "corrupt", resolver))
        .collect()
}

/// Re-export shared button state.
pub type SelectedButton = crate::ui::helpers::StashCancelButton;
