//! Health modal data types — pure serializable structs for health-related modal views.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::mutations::file_ops::HardLinkMutation;
use crate::mutations::indexing::{DropDirectoryFromIndexMutation, DropFromIndexMutation};
use crate::db_types::Zone;
use crate::mutations::tag_edit::ApplyTagOpsMutation;
use crate::mutations::{Mutation, TagOp};
use crate::paths::PathResolver;
use crate::signals::data::ArtistNeedsPluralData;

// ============================================================================
// Missing File Resolution
// ============================================================================

/// A missing corpus file that can be restored from library.
///
/// The same inode exists in the files table (zone='library'), meaning we can
/// hard-link from library back to corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NonRestorableMissingFile {
    /// Corpus path where file was indexed
    pub corpus_path: String,
    /// Inode for files table cleanup (None for orphaned signals)
    pub inode: Option<i64>,
}

/// Cached data for the missing file resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    /// Generate DropFromIndex mutations for ALL missing files (restorable + non-restorable).
    ///
    /// Used when the operator prefers to drop everything (e.g. damaged files being
    /// re-generated) rather than restoring from library.
    pub fn drop_all_missing(&self) -> Vec<Mutation> {
        let from_restorable = self.restorable.iter().map(|f| {
            Mutation::DropFromIndex(DropFromIndexMutation {
                path: PathBuf::from(&f.corpus_path),
                inode: Some(f.inode),
                zone: Some("corpus".to_string()),
            })
        });
        let from_non_restorable = self.non_restorable.iter().map(|f| {
            Mutation::DropFromIndex(DropFromIndexMutation {
                path: PathBuf::from(&f.corpus_path),
                inode: f.inode,
                zone: Some("corpus".to_string()),
            })
        });
        from_restorable.chain(from_non_restorable).collect()
    }

    /// Generate HardLink mutations to restore restorable files from library to corpus.
    pub fn restore_mutations(&self, resolver: &PathResolver) -> Vec<Mutation> {
        self.restorable
            .iter()
            .map(|f| {
                let source = resolver.resolve(std::path::Path::new(&f.library_path));
                let destination = resolver.resolve(std::path::Path::new(&f.corpus_path));
                Mutation::HardLink(HardLinkMutation { source, destination })
            })
            .collect()
    }
}

// ============================================================================
// Missing Directory Resolution
// ============================================================================

/// Data for the missing directory resolution modal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MissingDirectoryModalData {
    /// Missing directory paths
    pub directories: Vec<String>,
}

impl MissingDirectoryModalData {
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

// ============================================================================
// Corrupt File Resolution
// ============================================================================

/// A corrupt corpus file (tag parse error or waveform decode failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CorruptFileEntry {
    /// Corpus path (relative)
    pub corpus_path: String,
    /// Inode (for DropFromIndex cleanup)
    pub inode: i64,
}

/// Cached data for the corrupt file resolution modal.
///
/// Loaded once when the modal opens. All renders use this cached data.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    /// Generate StashFromZone + DropFromIndex mutations for all corrupt files.
    pub fn stash_and_drop_mutations(
        &self,
        resolver: &crate::paths::PathResolver,
    ) -> Vec<Mutation> {
        self.files
            .iter()
            .flat_map(|f| {
                crate::mutations::builders::stash_file_mutations(
                    &f.corpus_path,
                    f.inode,
                    "corrupt",
                    resolver,
                )
            })
            .collect()
    }
}

// ============================================================================
// Subpar Duplicate Resolution
// ============================================================================

/// A subpar duplicate file ready for stashing.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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

    /// Generate StashFromZone + DropFromIndex mutations for all subpar files.
    pub fn stash_and_drop_mutations(
        &self,
        resolver: &crate::paths::PathResolver,
    ) -> Vec<Mutation> {
        self.files
            .iter()
            .flat_map(|f| {
                crate::mutations::builders::stash_file_mutations(
                    &f.corpus_path,
                    f.inode,
                    "subpar",
                    resolver,
                )
            })
            .collect()
    }
}

// ============================================================================
// Artist Needs Plural Resolution
// ============================================================================

/// A file with multi-valued ARTIST/ALBUMARTIST needing plural tag normalization.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtistNeedsPluralEntry {
    pub inode: i64,
    pub corpus_path: String,
    pub data: ArtistNeedsPluralData,
}

/// Cached data for the artist-needs-plural resolution modal.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ArtistNeedsPluralModalData {
    pub files: Vec<ArtistNeedsPluralEntry>,
}

impl ArtistNeedsPluralModalData {
    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }

    /// Generate tag ops to normalize all files: semicolon-join singular,
    /// copy individual values to plural tag.
    pub fn pluralize_mutations(&self) -> Vec<Mutation> {
        let mut all_ops: Vec<TagOp> = Vec::new();

        for file in &self.files {
            if file.data.needs_artist && file.data.artist_values.len() > 1 {
                // Drop all existing ARTIST values
                for val in &file.data.artist_values {
                    all_ops.push(TagOp::drop_tag(file.inode, "ARTIST", val));
                }
                // Add semicolon-joined singular ARTIST
                let joined = file.data.artist_values.join("; ");
                all_ops.push(TagOp::add_tag(file.inode, "ARTIST", joined));
                // Add individual ARTISTS plural entries
                for val in &file.data.artist_values {
                    all_ops.push(TagOp::add_tag(file.inode, "ARTISTS", val));
                }
            }

            if file.data.needs_album_artist && file.data.album_artist_values.len() > 1 {
                for val in &file.data.album_artist_values {
                    all_ops.push(TagOp::drop_tag(file.inode, "ALBUMARTIST", val));
                }
                let joined = file.data.album_artist_values.join("; ");
                all_ops.push(TagOp::add_tag(file.inode, "ALBUMARTIST", joined));
                for val in &file.data.album_artist_values {
                    all_ops.push(TagOp::add_tag(file.inode, "ALBUMARTISTS", val));
                }
            }
        }

        if all_ops.is_empty() {
            return Vec::new();
        }

        vec![Mutation::ApplyTagOps(ApplyTagOpsMutation {
            ops: all_ops,
            zone: Zone::Corpus,
        })]
    }
}
