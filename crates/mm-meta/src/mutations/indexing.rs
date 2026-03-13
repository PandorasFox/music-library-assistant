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

// ============================================================================
// diff_entries implementations
// ============================================================================

use super::types::DiffEntry;

impl IndexFileFromPathMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "index",
            "",
            format!("{} ({})", self.path.display(), self.zone),
        )]
    }
}

impl UpdateFilePathMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        let mut entries = vec![DiffEntry::new(
            format!("[{}] path", self.inode),
            "",
            self.new_path.display(),
        )];
        if let Some(ref new_zone) = self.new_zone {
            entries.push(DiffEntry::new(
                format!("[{}] zone", self.inode),
                &self.zone,
                new_zone,
            ));
        }
        entries
    }
}

impl DropFromIndexMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new("drop", self.path.display(), "(removed)")]
    }
}

impl DropDirectoryFromIndexMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "drop dir",
            self.directory_path.display(),
            "(removed)",
        )]
    }
}

impl AcknowledgeMtimeOnlyMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "acknowledge mtime",
            "",
            format!("{} tracks", self.tracks.len()),
        )]
    }
}

impl ApplyDbTagsToDiskMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "DB→disk",
            "",
            self.path.display(),
        )]
    }
}

impl AssimilateDiskTagsToDbMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "disk→DB",
            "",
            self.path.display(),
        )]
    }
}

impl FlushTagsToDiskMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "flush tags",
            "",
            self.path.display(),
        )]
    }
}

impl EmitCanonicalTagMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            &self.tag_name,
            "",
            &self.canonical_value,
        )]
    }
}

impl EmitExpectedOverlapMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "expect overlap",
            &self.source_a,
            &self.source_b,
        )]
    }
}

impl EmitExpectedDuplicateMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "expect duplicate",
            "",
            &self.fingerprint_key,
        )]
    }
}

impl EmitExpectedMissingTagMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "expect missing tag",
            "",
            format!("{} inodes", self.inodes.len()),
        )]
    }
}

impl DropExternalMatchMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        vec![DiffEntry::new(
            "drop external match",
            format!("inode {}", self.inode),
            "(removed)",
        )]
    }
}
