//! Missing Directory Resolution Modal
//!
//! Provides the interactive workflow for acknowledging and dropping missing directories
//! from the index. When a directory is deleted externally, this modal allows the operator
//! to acknowledge the deletion and remove the directory and its contents from the index.

pub mod preview;

pub use preview::{MissingDirectoryPreviewAction, MissingDirectoryPreviewState};

use crate::db::ReadOnlyDb;
use crate::meta::mutations::indexing::DropDirectoryFromIndexMutation;
use crate::meta::mutations::Mutation;
use std::path::PathBuf;

/// Data for the missing directory resolution modal.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MissingDirectoryModalData {
    /// Missing directory paths
    pub directories: Vec<String>,
}

impl MissingDirectoryModalData {
    /// Load missing directory data from the database.
    pub fn load(read_db: &ReadOnlyDb<'_>) -> anyhow::Result<Self> {
        let directories = read_db.get_missing_directory_paths()?;
        Ok(Self { directories })
    }

    /// Count of missing directories.
    pub fn count(&self) -> usize {
        self.directories.len()
    }

    /// Generate mutations to drop all missing directories from the index.
    pub fn drop_mutations(&self) -> Vec<Mutation> {
        self.directories
            .iter()
            .map(|dir| {
                Mutation::DropDirectoryFromIndex(DropDirectoryFromIndexMutation {
                    directory_path: PathBuf::from(dir),
                })
            })
            .collect()
    }
}
