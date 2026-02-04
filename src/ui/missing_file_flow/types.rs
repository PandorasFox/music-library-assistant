//! Missing File Resolution Modal Types
//!
//! Data structures for the missing file resolution modal, including
//! categorized files (restorable vs non-restorable) and button state.

use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;

use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;

/// A missing corpus file that can be restored from library.
///
/// The same inode exists in the files table (source='library'), meaning we can
/// hard-link from library back to corpus.
#[derive(Debug, Clone)]
pub struct RestorableMissingFile {
    /// Corpus path where file should exist
    pub corpus_path: String,
    /// Track ID from tracks table
    pub _track_id: i64,
    /// Inode of the missing file
    pub _inode: i64,
    /// Library path where the same inode exists (restore source)
    pub library_path: String,
}

/// A missing corpus file that cannot be restored.
///
/// The inode doesn't exist in any library - data is gone.
#[derive(Debug, Clone)]
pub struct NonRestorableMissingFile {
    /// Corpus path where file was indexed
    pub corpus_path: String,
    /// Track ID from tracks table
    pub track_id: i64,
    /// Inode (for DropFromIndex cleanup)
    pub inode: i64,
}

/// Cached data for the missing file resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default)]
pub struct MissingFileModalData {
    /// Files that can be restored (inode exists in library)
    pub restorable: Vec<RestorableMissingFile>,
    /// Files that cannot be restored (data is gone)
    pub non_restorable: Vec<NonRestorableMissingFile>,
}

impl MissingFileModalData {
    /// Load and categorize missing files from the database.
    ///
    /// A file is restorable if its inode exists in the files table (source='library').
    /// For missing files, the file doesn't exist on disk. We try to get the inode
    /// from the database (files or audio_info tables).
    pub fn load(read_db: &ReadOnlyDb<'_>) -> Result<Self> {
        // Step 1: Get all MissingFile signals (issue_key = corpus path)
        let missing_paths = read_db.get_missing_file_paths()?;

        if missing_paths.is_empty() {
            return Ok(Self::default());
        }

        // Step 2: Build inode -> library_path map from files table (library source)
        let library_files = read_db.get_all_library_files()?;
        let inode_to_library: HashMap<i64, String> = library_files
            .into_iter()
            .map(|e| (e.inode, e.file_path.to_string_lossy().to_string()))
            .collect();

        // Step 3: Categorize each missing file
        let mut restorable = Vec::new();
        let mut non_restorable = Vec::new();

        for corpus_path in missing_paths {
            // Try to get file info - first try audio_file (files+audio_info join)
            let inode = if let Some(af) = read_db.get_audio_file_by_path(&corpus_path)? {
                Some(af.inode())
            } else if let Some(fe) = read_db.get_file_entry_by_path(&corpus_path, "corpus")? {
                // Fallback: files table entry without audio_info (rare but possible)
                Some(fe.inode)
            } else {
                // No database entry found - this is an orphaned signal
                // (e.g., file was stashed/dropped, signal not cleared yet)
                // Still show it so user can clear the stale signal
                None
            };

            if let Some(inode) = inode {
                // We have an inode - check if restorable from library
                if let Some(library_path) = inode_to_library.get(&inode) {
                    restorable.push(RestorableMissingFile {
                        corpus_path,
                        _track_id: inode,
                        _inode: inode,
                        library_path: library_path.clone(),
                    });
                } else {
                    non_restorable.push(NonRestorableMissingFile {
                        corpus_path,
                        track_id: inode,
                        inode,
                    });
                }
            } else {
                // Orphaned signal - no DB entry, just needs signal cleared
                // Use inode=-1 as sentinel for "clear signal only"
                non_restorable.push(NonRestorableMissingFile {
                    corpus_path,
                    track_id: -1,
                    inode: -1,
                });
            }
        }

        Ok(Self {
            restorable,
            non_restorable,
        })
    }

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
    /// - library_path: relative to libraries_root
    /// - corpus_path: relative to corpus_root
    /// These must be resolved to absolute for HardLink filesystem operations.
    pub fn restore_mutations(&self) -> Vec<crate::corpus::mutations::Mutation> {
        let resolver = paths::get_resolver();
        self.restorable
            .iter()
            .filter_map(|f| {
                // Resolve relative paths to absolute
                let source = resolver.resolve(std::path::Path::new(&f.library_path));
                let destination = resolver.resolve(std::path::Path::new(&f.corpus_path));
                Some(crate::corpus::mutations::Mutation::HardLink {
                    source,
                    destination,
                })
            })
            .collect()
    }

    /// Generate DropFromIndex mutations for non-restorable files.
    pub fn drop_mutations(&self) -> Vec<crate::corpus::mutations::Mutation> {
        self.non_restorable
            .iter()
            .map(|f| crate::corpus::mutations::Mutation::DropFromIndex {
                track_id: f.track_id,
                path: PathBuf::from(&f.corpus_path),
                inode: Some(f.inode),
                source: Some("corpus".to_string()),
            })
            .collect()
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
    pub fn left(&mut self, has_restorable: bool, has_non_restorable: bool) {
        *self = match *self {
            Self::Cancel => {
                if has_non_restorable {
                    Self::DropLost
                } else if has_restorable {
                    Self::RestoreAll
                } else {
                    Self::Cancel
                }
            }
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
    pub fn right(&mut self, has_restorable: bool, has_non_restorable: bool) {
        *self = match *self {
            Self::RestoreAll => {
                if has_non_restorable {
                    Self::DropLost
                } else {
                    Self::Cancel
                }
            }
            Self::DropLost => Self::Cancel,
            Self::Cancel => Self::Cancel,
        };
        // Ensure we don't land on disabled buttons
        if *self == Self::RestoreAll && !has_restorable {
            self.right(has_restorable, has_non_restorable);
        }
        if *self == Self::DropLost && !has_non_restorable {
            self.right(has_restorable, has_non_restorable);
        }
    }
}
