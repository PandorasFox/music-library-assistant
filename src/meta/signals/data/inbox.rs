//! Inbox signal types.

use serde::{Deserialize, Serialize};
use std::hash::Hash;

use super::{CompoundTagEntry, MissingTagData};
use crate::meta::signals::registry::SignalContentHash;

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

/// Quality classification of an inbox file relative to its corpus matches.
///
/// Computed at signal emission time by comparing quality tiers (format class,
/// bitrate, sample rate). Stored both in the bincode BLOB and as a SQL column
/// for efficient query filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CorpusMatchQuality {
    /// Inbox file is better quality than all corpus matches — should be organized in.
    Better,
    /// Same quality tier as best corpus match — safe to stash.
    Equivalent,
    /// Inbox file is lower quality — safe to stash.
    Subpar,
}

impl_as_str!(CorpusMatchQuality, as_str, [
    Better => "better",
    Equivalent => "equivalent",
    Subpar => "subpar",
]);

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

/// Match details for an inbox file that overlaps with corpus.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxCorpusMatchData {
    /// Corpus inodes that match this inbox file.
    pub corpus_matches: Vec<InboxCorpusMatch>,
    /// Pre-computed quality classification (also stored as SQL column).
    pub classification: CorpusMatchQuality,
}

/// A single corpus file matching an inbox file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxCorpusMatch {
    pub corpus_inode: i64,
    pub corpus_path: String,
    pub similarity: f64,
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

/// Bincode-serialized payload for InboxTagCanonicity.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InboxTagCanonicityData {
    /// Inbox variants not matching any corpus value: (value, count)
    pub inbox_variants: Vec<(String, usize)>,
    /// All inbox inodes affected
    pub inbox_inodes: Vec<i64>,
    /// Corpus variants for this normalized key: (value, count) sorted DESC
    pub corpus_variants: Vec<(String, usize)>,
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
