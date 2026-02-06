//! Types for compound tag split resolution modal.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::corpus::db::types::AggregateSignal;
use crate::corpus::mutations::{Mutation, TagOp};
use crate::corpus::tags::TagSet;

// ============================================================================
// Data Types (loaded from signal)
// ============================================================================

/// Data for a compound tag split resolution.
///
/// Loaded from a CompoundTagValue signal's metadata JSON.
#[derive(Debug, Clone)]
pub struct CompoundSplitData {
    /// Tag name (e.g., "genre", "artist")
    pub tag_name: String,
    /// Original compound value (e.g., "Rock; Metal" or "Priority & TwoThirds")
    pub compound_value: String,
    /// Detected separator (e.g., "; " or " & ")
    pub _separator: String,
    /// Split parts (e.g., ["Rock", "Metal"] or ["Priority", "TwoThirds"])
    pub split_parts: Vec<String>,
    /// Inodes affected by this compound value
    pub inodes: Vec<i64>,
    /// For artist tag: which split parts exist as standalone artists in corpus.
    /// Empty list = no matches → suggests canonicalization (band name).
    /// Non-empty = known artists → suggests splitting (collaboration).
    pub matching_parts: Vec<String>,
}

impl CompoundSplitData {
    /// Parse from an AggregateSignal's metadata JSON.
    pub fn from_signal(signal: &AggregateSignal) -> Option<Self> {
        let metadata = signal.metadata_json.as_ref()?;
        let json: serde_json::Value = serde_json::from_str(metadata).ok()?;

        let tag_name = json.get("tag_name")?.as_str()?.to_string();
        let compound_value = json.get("compound_value")?.as_str()?.to_string();
        let separator = json.get("separator")?.as_str()?.to_string();

        let split_parts: Vec<String> = json
            .get("split_parts")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect();

        let inodes: Vec<i64> = json
            .get("inodes")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_i64())
            .collect();

        // Parse matching_parts (only present for artist tag)
        let matching_parts: Vec<String> = json
            .get("matching_parts")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        Some(Self {
            tag_name,
            compound_value,
            _separator: separator,
            split_parts,
            inodes,
            matching_parts,
        })
    }

    /// Whether this is an artist tag with known standalone parts (suggests splitting).
    pub fn suggests_split(&self) -> bool {
        self.tag_name.to_lowercase() == "artist" && !self.matching_parts.is_empty()
    }

    /// Whether this is an artist tag with NO known standalone parts (suggests canonicalizing).
    pub fn suggests_canonicalize(&self) -> bool {
        self.tag_name.to_lowercase() == "artist" && self.matching_parts.is_empty()
    }

    /// Create a CanonicalTag signal emission mutation.
    ///
    /// Called when user chooses to mark this value as canonical (not split).
    pub fn create_canonical_signal(&self) -> Mutation {
        // The CanonicalTag signal will be emitted by a computation after the
        // mutation completes. We use a marker mutation to trigger this.
        Mutation::EmitCanonicalTag {
            tag_name: self.tag_name.clone(),
            canonical_value: self.compound_value.clone(),
        }
    }
}

// ============================================================================
// State
// ============================================================================

/// State for the compound split resolution modal.
#[derive(Debug, Clone)]
pub struct CompoundSplitState {
    /// Data loaded from the signal
    pub data: CompoundSplitData,
    /// Current cursor position in the split parts list
    pub cursor: usize,
    /// Index of current signal in the cluster (for "1/5" display)
    pub group_index: usize,
    /// Total number of signals in the cluster
    pub total_groups: usize,
}

impl CompoundSplitState {
    pub fn new(data: CompoundSplitData, group_index: usize, total_groups: usize) -> Self {
        Self {
            data,
            cursor: 0,
            group_index,
            total_groups,
        }
    }

    /// Move cursor up in the split parts list.
    pub fn cursor_up(&mut self) {
        if self.cursor > 0 {
            self.cursor -= 1;
        }
    }

    /// Move cursor down in the split parts list.
    pub fn cursor_down(&mut self) {
        if self.cursor < self.data.split_parts.len().saturating_sub(1) {
            self.cursor += 1;
        }
    }

    /// Generate mutations for this split using incremental TagOps.
    ///
    /// For each affected track:
    /// - Verify the track still has the compound value (skip if not)
    /// - Generate TagOps: drop compound value, add each split part
    ///
    /// The `track_info` map contains (path, current_tagset) for each track.
    /// This allows verifying the track actually has the compound value before
    /// generating mutations, preventing no-op mutations when the value has
    /// already been fixed or changed.
    ///
    /// Returns a single ApplyTagOps mutation containing ops for all affected tracks.
    pub fn mutations_with_paths(
        &self,
        track_info: &HashMap<i64, (PathBuf, TagSet)>,
    ) -> Vec<Mutation> {
        let mut ops = Vec::new();

        for &inode in &self.data.inodes {
            let Some((_path, current_tagset)) = track_info.get(&inode) else {
                continue;
            };

            // Verify file still has the compound value
            if !current_tagset.contains(&self.data.tag_name, &self.data.compound_value) {
                continue; // Already fixed or changed, skip this file
            }

            // Generate TagOps: drop compound value, add each split part
            // First part replaces the compound value
            if let Some(first_part) = self.data.split_parts.first() {
                ops.push(TagOp::replace_tag(
                    inode,
                    &self.data.tag_name,
                    &self.data.compound_value,
                    first_part,
                ));
            }

            // Additional parts are added
            for part in self.data.split_parts.iter().skip(1) {
                ops.push(TagOp::add_tag(inode, &self.data.tag_name, part));
            }
        }

        if ops.is_empty() {
            Vec::new()
        } else {
            vec![Mutation::ApplyTagOps { ops }]
        }
    }
}

// ============================================================================
// Cluster Navigation
// ============================================================================

/// Tracks navigation through compound tag split signals.
///
/// Similar to TagCanonicityClusters but for compound values.
#[derive(Debug, Clone)]
pub struct CompoundSplitClusters {
    /// Signal IDs in display order
    signal_ids: Vec<i64>,
    /// Current index
    current: usize,
}

impl CompoundSplitClusters {
    pub fn new(signal_ids: Vec<i64>) -> Self {
        Self {
            signal_ids,
            current: 0,
        }
    }

    /// Get the current signal ID.
    pub fn current_signal_id(&self) -> Option<i64> {
        self.signal_ids.get(self.current).copied()
    }

    /// Get current index (0-based).
    pub fn current_index(&self) -> usize {
        self.current
    }

    /// Get total count.
    pub fn total(&self) -> usize {
        self.signal_ids.len()
    }

    /// Get all signal IDs.
    pub fn all_signal_ids(&self) -> &[i64] {
        &self.signal_ids
    }

    /// Check if at the last signal.
    pub fn is_last(&self) -> bool {
        self.current >= self.signal_ids.len().saturating_sub(1)
    }

    /// Check if at the first signal.
    pub fn is_first(&self) -> bool {
        self.current == 0
    }

    /// Move to next signal. Returns true if moved, false if already at end.
    pub fn next(&mut self) -> bool {
        if self.current < self.signal_ids.len().saturating_sub(1) {
            self.current += 1;
            true
        } else {
            false
        }
    }

    /// Move to previous signal. Returns true if moved, false if already at start.
    pub fn prev(&mut self) -> bool {
        if self.current > 0 {
            self.current -= 1;
            true
        } else {
            false
        }
    }

}
