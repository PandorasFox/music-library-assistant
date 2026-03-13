//! Inbox signal data types.

use serde::{Deserialize, Serialize};

#[allow(unused_imports)]
use super::impl_as_str;

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
