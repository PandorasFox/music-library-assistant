//! File operation mutation structs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Move a file from source to destination.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MoveMutation {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// Stash a corpus or inbox file (operator-driven eviction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashFromZoneMutation {
    pub path: PathBuf,
    pub stash_name: String,
}

/// Stash orphaned library files during deploy cleanup.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StashLeftoversMutation {
    pub path: PathBuf,
}

/// Create a hard link from source to destination (for deployment).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HardLinkMutation {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// Move a file within a library.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LibraryMoveMutation {
    pub source: PathBuf,
    pub destination: PathBuf,
}

/// Move an inbox file into the corpus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxToCorpusMutation {
    pub inode: i64,
    pub inbox_path: PathBuf,
    pub corpus_path: PathBuf,
}

/// A tracked audio file within an inbox directory being emplaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxDirTrackedFile {
    pub inode: i64,
    pub corpus_path: PathBuf,
}

/// Move an entire inbox directory into the corpus.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InboxDirToCorpusMutation {
    pub inbox_dir_path: PathBuf,
    pub corpus_dir_path: PathBuf,
    pub tracked_files: Vec<InboxDirTrackedFile>,
}

// ============================================================================
// diff_entries implementations
// ============================================================================

use super::types::DiffEntry;

impl MoveMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new("path", self.source.display(), self.destination.display())]
    }
}

impl StashFromZoneMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "stash",
            self.path.display(),
            &self.stash_name,
        )]
    }
}

impl StashLeftoversMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new("stash", self.path.display(), "(stashed)")]
    }
}

impl HardLinkMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new("link", self.source.display(), self.destination.display())]
    }
}

impl LibraryMoveMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new("path", self.source.display(), self.destination.display())]
    }
}

impl InboxToCorpusMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "path",
            self.inbox_path.display(),
            self.corpus_path.display(),
        )]
    }
}

impl InboxDirToCorpusMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        let mut entries = vec![DiffEntry::new(
            "directory",
            self.inbox_dir_path.display(),
            self.corpus_dir_path.display(),
        )];
        for f in &self.tracked_files {
            entries.push(DiffEntry::new(
                format!("[{}]", f.inode),
                "",
                f.corpus_path.display(),
            ));
        }
        entries
    }
}
