//! Missing File Resolution Modal Types
//!
//! Data structures for the missing file resolution modal, including
//! categorized files (restorable vs non-restorable) and button state.

use std::path::PathBuf;

use crate::corpus::paths;
use crate::meta::mutations::indexing::DropFromIndexMutation;

/// A missing corpus file that can be restored from library.
///
/// The same inode exists in the files table (zone='library'), meaning we can
/// hard-link from library back to corpus.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RestorableMissingFile {
    /// Corpus path where file should exist
    pub corpus_path: String,
    /// Library path where the same inode exists (restore source)
    pub library_path: String,
    /// Inode for the file (needed if operator chooses drop instead of restore)
    pub inode: i64,
}

/// A missing corpus file that cannot be restored.
///
/// The inode doesn't exist in any library - data is gone.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct NonRestorableMissingFile {
    /// Corpus path where file was indexed
    pub corpus_path: String,
    /// Inode for files table cleanup (None for orphaned signals)
    pub inode: Option<i64>,
}

/// Cached data for the missing file resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct MissingFileModalData {
    /// Files that can be restored (inode exists in library)
    pub restorable: Vec<RestorableMissingFile>,
    /// Files that cannot be restored (data is gone)
    pub non_restorable: Vec<NonRestorableMissingFile>,
}

impl MissingFileModalData {
    /// Total number of missing files.
    pub fn total_count(&self) -> usize {
        self.restorable.len() + self.non_restorable.len()
    }

    /// Check if there's anything to restore.
    pub fn has_restorable(&self) -> bool {
        !self.restorable.is_empty()
    }

    /// Check if there are non-restorable files.
    pub fn has_non_restorable(&self) -> bool {
        !self.non_restorable.is_empty()
    }

    /// Generate HardLink mutations for restorable files.
    ///
    /// Paths stored in RestorableMissingFile are relative to their roots:
    ///   - library_path: relative to libraries_root
    ///   - corpus_path: relative to corpus_root
    ///
    /// These must be resolved to absolute for HardLink filesystem operations.
    pub fn restore_mutations(&self) -> Vec<crate::meta::mutations::Mutation> {
        let resolver = paths::get_resolver();
        self.restorable
            .iter()
            .map(|f| {
                // Resolve relative paths to absolute
                let source = resolver.resolve(std::path::Path::new(&f.library_path));
                let destination = resolver.resolve(std::path::Path::new(&f.corpus_path));
                crate::meta::mutations::Mutation::HardLink(
                    crate::meta::mutations::file_ops::HardLinkMutation {
                        source,
                        destination,
                    },
                )
            })
            .collect()
    }

    /// Generate DropFromIndex mutations for ALL missing files (restorable + non-restorable).
    ///
    /// Used when the operator prefers to drop everything (e.g. damaged files being
    /// re-generated) rather than restoring from library.
    pub fn drop_all_missing(&self) -> Vec<crate::meta::mutations::Mutation> {
        let from_restorable = self.restorable.iter().map(|f| {
            crate::meta::mutations::Mutation::DropFromIndex(DropFromIndexMutation {
                path: PathBuf::from(&f.corpus_path),
                inode: Some(f.inode),
                zone: Some("corpus".to_string()),
            })
        });
        let from_non_restorable = self.non_restorable.iter().map(|f| {
            crate::meta::mutations::Mutation::DropFromIndex(DropFromIndexMutation {
                path: PathBuf::from(&f.corpus_path),
                inode: f.inode,
                zone: Some("corpus".to_string()),
            })
        });
        from_restorable.chain(from_non_restorable).collect()
    }
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
