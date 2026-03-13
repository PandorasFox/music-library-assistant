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
