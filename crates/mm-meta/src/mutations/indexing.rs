//! Indexing mutation structs.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::db_types::Zone;
use crate::tags::TagSet;

/// Index a file from path only - extracts metadata during execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndexFileFromPathMutation {
    pub path: PathBuf,
    pub zone: String,
}

/// Update file path in files table (for relocated files).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UpdateFilePathMutation {
    pub zone: String,
    pub inode: i64,
    pub new_path: PathBuf,
    /// If set, update the zone column to this value (cross-zone move).
    pub new_zone: Option<String>,
}

/// Drop file from index (for missing files or orphaned signals).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropFromIndexMutation {
    pub path: PathBuf,
    /// Inode to also remove from files table (None for orphaned signals)
    pub inode: Option<i64>,
    pub zone: Option<String>,
}

/// Drop a directory and all its contents from the index.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropDirectoryFromIndexMutation {
    /// Relative path of the directory
    pub directory_path: PathBuf,
}

/// Acknowledge mtime-only change.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcknowledgeMtimeOnlyMutation {
    /// Inodes with their absolute paths: (inode, abs_path)
    pub tracks: Vec<(i64, PathBuf)>,
}

/// Apply DB tags to disk file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyDbTagsToDiskMutation {
    pub inode: i64,
    pub path: PathBuf,
    /// Zone determines which tag table to read from.
    pub zone: Zone,
}

/// Assimilate disk tags into DB.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssimilateDiskTagsToDbMutation {
    pub inode: i64,
    pub path: PathBuf,
    /// File zone, carried in-band when chain-spawned from Transcode.
    pub zone: Option<String>,
}

/// Flush committed DB tags to disk, with validation against expected state.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FlushTagsToDiskMutation {
    pub inode: i64,
    pub path: PathBuf,
    pub expected_tags: TagSet,
    /// Zone determines which tag table to read from for validation.
    pub zone: Zone,
}

/// Emit a CanonicalTag signal to whitelist a tag value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitCanonicalTagMutation {
    pub tag_name: String,
    pub canonical_value: String,
}

/// Mark a source pair overlap as expected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitExpectedOverlapMutation {
    pub source_a: String,
    pub source_b: String,
}

/// Mark a fingerprint overlap group as expected.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitExpectedDuplicateMutation {
    pub fingerprint_key: String,
}

/// Mark inodes as expected-missing-tag.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EmitExpectedMissingTagMutation {
    pub inodes: Vec<i64>,
}

/// Drop external match data for an inode.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropExternalMatchMutation {
    pub inode: i64,
}
