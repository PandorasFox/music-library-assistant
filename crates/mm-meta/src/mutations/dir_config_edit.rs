//! Dir config edit mutation structs.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::config::{Config, SourceDir};

/// Mutation that applies a single source directory config edit to dirs.kdl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyDirConfigEditMutation {
    /// Which source dir was edited (relative path within corpus).
    pub source_path: PathBuf,
    /// The config as it was before editing (for diffing).
    pub old_dir: SourceDir,
    /// The new config with edits applied.
    pub new_dir: SourceDir,
    /// Full config with the dir edit applied, for SharedConfig update.
    pub new_config: Config,
}

impl PartialEq for ApplyDirConfigEditMutation {
    fn eq(&self, other: &Self) -> bool {
        self.source_path == other.source_path
    }
}

/// A single dir config edit entry within a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirConfigEditEntry {
    pub source_path: PathBuf,
    pub old_dir: SourceDir,
    pub new_dir: SourceDir,
}

/// Batch mutation that atomically applies multiple dir config edits to dirs.kdl.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyBatchDirConfigEditsMutation {
    pub edits: Vec<DirConfigEditEntry>,
    /// Full config with all edits applied, for SharedConfig update.
    pub new_config: Config,
}

impl PartialEq for ApplyBatchDirConfigEditsMutation {
    fn eq(&self, other: &Self) -> bool {
        self.edits.len() == other.edits.len()
            && self
                .edits
                .iter()
                .zip(other.edits.iter())
                .all(|(a, b)| a.source_path == b.source_path)
    }
}
