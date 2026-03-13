//! Subpar Duplicate Resolution Modal Types
//!
//! Data structures for the subpar duplicate resolution modal, including
//! file entries and button state.

pub use mm_meta::views::health_modals::{SubparDuplicateModalData, SubparFileEntry};

use mm_meta::mutations::Mutation;
use crate::helpers::stash_file_mutations;

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

/// Re-export shared button state.
pub type SelectedButton = crate::helpers::StashCancelButton;
