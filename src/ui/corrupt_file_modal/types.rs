//! Corrupt File Resolution Modal Types
//!
//! Data structures for the corrupt file resolution modal, including
//! file entries and button state.

use crate::meta::mutations::Mutation;
use crate::ui::helpers::stash_file_mutations;

/// A corrupt corpus file (tag parse error or waveform decode failure).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct CorruptFileEntry {
    /// Corpus path (relative)
    pub corpus_path: String,
    /// Inode (for DropFromIndex cleanup)
    pub inode: i64,
}

/// Cached data for the corrupt file resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct CorruptFileModalData {
    /// Files with CorruptFile signals
    pub files: Vec<CorruptFileEntry>,
}

impl CorruptFileModalData {
    /// Total number of corrupt files.
    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    /// Check if there are any corrupt files.
    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Generate StashFromZone + DropFromIndex mutations for all files.
    pub fn stash_and_drop_mutations(
        &self,
        resolver: &crate::corpus::paths::PathResolver,
    ) -> Vec<Mutation> {
        self.files
            .iter()
            .flat_map(|f| stash_file_mutations(&f.corpus_path, f.inode, "corrupt", resolver))
            .collect()
    }
}

/// Re-export shared button state.
pub type SelectedButton = crate::ui::helpers::StashCancelButton;
