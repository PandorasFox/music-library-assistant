//! Subpar Duplicate Resolution Modal Types
//!
//! Data structures for the subpar duplicate resolution modal, including
//! file entries and button state.

use crate::meta::mutations::Mutation;
use crate::ui::helpers::stash_file_mutations;

/// A subpar duplicate file ready for stashing.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SubparFileEntry {
    /// Corpus path (the subpar file)
    pub corpus_path: String,
    /// Inode (for DropFromIndex cleanup)
    pub inode: i64,
    /// Reason for being subpar (human-readable)
    pub reason: String,
    /// Path to the superior version
    pub superior_path: String,
    /// Fingerprint similarity score (0.0-100.0).
    pub similarity_score: f64,
}

/// Cached data for the subpar duplicate resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SubparDuplicateModalData {
    /// Files with SubparDuplicate signals
    pub files: Vec<SubparFileEntry>,
}

impl SubparDuplicateModalData {
    /// Total number of subpar files.
    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    /// Check if there are any subpar files.
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
            .flat_map(|f| stash_file_mutations(&f.corpus_path, f.inode, "subpar", resolver))
            .collect()
    }
}

/// Re-export shared button state.
pub type SelectedButton = crate::ui::helpers::StashCancelButton;
