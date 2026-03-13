//! Subpar Duplicate Resolution Modal Types
//!
//! Data structures for the subpar duplicate resolution modal, including
//! file entries and button state.

use std::os::unix::fs::MetadataExt;

use anyhow::Result;

use crate::corpus::paths;
use crate::db::ReadOnlyDb;
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
    /// Load subpar duplicate files from the database.
    ///
    /// Subpar duplicate files exist on disk but are lower quality versions of other files.
    /// We try to get the inode from the database first, then fall back to filesystem lookup.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Get all SubparDuplicate signals with metadata
        let subpar_entries = read_db.get_subpar_duplicate_files()?;

        if subpar_entries.is_empty() {
            return Ok(Self::default());
        }

        let resolver = paths::get_resolver();
        let mut files = Vec::new();

        for entry in subpar_entries {
            // Try to get inode from database first
            let inode = if let Some(af) = read_db.get_audio_file_by_path(&entry.corpus_path)? {
                af.inode()
            } else if let Some(fe) = read_db.get_file_entry_by_path(&entry.corpus_path, "corpus")? {
                fe.inode
            } else {
                // Fall back to filesystem lookup (subpar files should exist on disk)
                let abs_path = resolver.resolve(std::path::Path::new(&entry.corpus_path));
                if let Ok(metadata) = std::fs::metadata(&abs_path) {
                    metadata.ino() as i64
                } else {
                    // File doesn't exist on disk - skip it
                    continue;
                }
            };

            // Convert reason to human-readable
            let reason = match entry.reason.as_str() {
                "SubparBitrate" => "Lower bitrate".to_string(),
                "SubparFormat" => "Worse format".to_string(),
                "SubparSampleRate" => "Lower sample rate".to_string(),
                other => other.to_string(),
            };

            files.push(SubparFileEntry {
                corpus_path: entry.corpus_path,
                inode,
                reason,
                superior_path: entry.superior_path,
                similarity_score: entry.similarity_score,
            });
        }

        Ok(Self { files })
    }

    /// Total number of subpar files.
    pub fn total_count(&self) -> usize {
        self.files.len()
    }

    /// Check if there are any subpar files.
    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Generate StashFromZone + DropFromIndex mutations for all files.
    pub fn stash_and_drop_mutations(&self) -> Vec<Mutation> {
        self.files
            .iter()
            .flat_map(|f| stash_file_mutations(&f.corpus_path, f.inode, "subpar"))
            .collect()
    }
}

/// Re-export shared button state.
pub type SelectedButton = crate::ui::helpers::StashCancelButton;
