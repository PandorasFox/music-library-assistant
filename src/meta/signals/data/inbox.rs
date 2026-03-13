//! Inbox signal types.

use std::hash::Hash;

use super::{CompoundTagEntry, MissingTagData};
use crate::meta::signals::registry::SignalContentHash;

// Re-export pure data types from mm-meta
pub use mm_meta::signals::data::{
    CorpusMatchQuality, InboxCorpusMatchData, InboxTagCanonicityData,
};

// ============================================================================
// Inbox file signals (inode-keyed)
// ============================================================================

/// File exists in inbox directory.
/// Emitted during inbox walk (Observation phase).
#[derive(Debug, Clone)]
pub struct FileInInboxSignal {
    pub inode: i64,
    pub path: String,
    /// Observation generation for stale signal cleanup.
    pub generation: u8,
}

/// File in inbox but not in index (needs indexing).
#[derive(Debug, Clone)]
pub struct InboxUnindexedSignal {
    pub inode: i64,
    pub path: String,
}

/// File in inbox + index with matching state (healthy).
#[derive(Debug, Clone)]
pub struct InboxHealthySignal {
    pub inode: i64,
    pub path: String,
}

/// Inbox file has fingerprint+duration match against corpus file(s).
/// Likely a duplicate — operator can stash the inbox copy.
#[derive(Debug, Clone)]
pub struct InboxCorpusMatchSignal {
    pub inode: i64,
    pub path: String,
    /// Pre-computed quality classification relative to best corpus match.
    pub classification: CorpusMatchQuality,
    /// Serialized as bincode BLOB.
    pub data: InboxCorpusMatchData,
}

/// Inbox files missing required tags.
/// Aggregate signal keyed by album/directory, reuses MissingTagData.
#[derive(Debug, Clone)]
pub struct InboxMissingTagSignal {
    pub key: String,
    /// Serialized as bincode BLOB.
    pub data: MissingTagData,
}

/// Per-inbox-file compound tag detection results.
/// Reuses Vec<CompoundTagEntry> from corpus compound tag detection.
#[derive(Debug, Clone)]
pub struct InboxCompoundTagSignal {
    pub inode: i64,
    pub path: String,
    /// Serialized as bincode BLOB.
    pub compounds: Vec<CompoundTagEntry>,
}

/// Inbox tag values that differ from corpus canonical spellings.
/// Aggregate signal keyed by "{tag_name}:{normalized_key}".
#[derive(Debug, Clone)]
pub struct InboxTagCanonicitySignal {
    pub key: String,      // "artist:beyonce"
    pub tag_name: String, // "artist"
    /// Serialized as bincode BLOB.
    pub data: InboxTagCanonicityData,
}

// ============================================================================
// impl_content_hash! invocations for inbox signals
// ============================================================================

impl_content_hash!(FileInInboxSignal => [path]);
impl_content_hash!(InboxUnindexedSignal => [path]);
impl_content_hash!(InboxHealthySignal => [path]);
impl_content_hash!(InboxCorpusMatchSignal => blob(data));
impl_content_hash!(InboxCompoundTagSignal => blob(compounds));
impl_content_hash!(InboxTagCanonicitySignal => blob(data));
impl_content_hash!(InboxMissingTagSignal => blob(data));
