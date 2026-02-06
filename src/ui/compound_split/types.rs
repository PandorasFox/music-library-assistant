//! Types for compound tag split resolution modal.

use std::collections::HashMap;
use std::path::PathBuf;

use crate::corpus::db::types::AggregateSignal;
use crate::corpus::mutations::Mutation;
use crate::corpus::tags::TagSet;

// ============================================================================
// Data Types (loaded from signal)
// ============================================================================

/// Data for a compound tag split resolution.
///
/// Loaded from a CompoundTagValue signal's metadata JSON.
#[derive(Debug, Clone)]
pub struct CompoundSplitData {
    /// Tag name (e.g., "genre")
    pub tag_name: String,
    /// Original compound value (e.g., "Rock; Metal")
    pub compound_value: String,
    /// Detected separator (e.g., "; ")
    pub _separator: String,
    /// Split parts (e.g., ["Rock", "Metal"])
    pub split_parts: Vec<String>,
    /// Inodes affected by this compound value
    pub inodes: Vec<i64>,
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

        Some(Self {
            tag_name,
            compound_value,
            _separator: separator,
            split_parts,
            inodes,
        })
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

    /// Generate mutations for this split using DB-first pattern with spawn chaining.
    ///
    /// For each affected track:
    /// - Verify the track still has the compound value (skip if not)
    /// - Build new TagSet with compound value replaced by split parts
    /// - Generate SetTrackTagsDb mutation (spawns ApplyDbTagsToDisk automatically)
    ///
    /// The `track_info` map contains (path, current_tagset) for each track.
    /// This allows verifying the track actually has the compound value before
    /// generating mutations, preventing no-op mutations when the value has
    /// already been fixed or changed.
    pub fn mutations_with_paths(
        &self,
        track_info: &HashMap<i64, (PathBuf, TagSet)>,
    ) -> Vec<Mutation> {
        let mut mutations = Vec::new();

        for &inode in &self.data.inodes {
            let Some((_path, current_tagset)) = track_info.get(&inode) else {
                continue;
            };

            // Verify file still has the compound value
            if !current_tagset.contains(&self.data.tag_name, &self.data.compound_value) {
                continue; // Already fixed or changed, skip this file
            }

            // Build new TagSet: remove compound value, add split parts
            let mut new_tags: Vec<(String, String)> = current_tagset
                .iter()
                .filter(|(k, v)| {
                    // Keep all tags EXCEPT the compound value being split
                    !(k.eq_ignore_ascii_case(&self.data.tag_name) && v == &self.data.compound_value)
                })
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();

            // Add split parts as separate values
            for part in &self.data.split_parts {
                new_tags.push((self.data.tag_name.clone(), part.clone()));
            }

            // Deduplicate via TagSet before creating mutation to prevent
            // UNIQUE constraint violations in the database
            let deduped = TagSet::new(new_tags.into_iter());
            let deduped_tags = deduped.into_vec();

            // DB-first pattern with spawn chaining:
            // SetTrackTagsDb writes to DB and spawns ApplyDbTagsToDisk for disk sync
            mutations.push(Mutation::SetTrackTagsDb {
                inode,
                tags: deduped_tags,
            });
        }

        mutations
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
