//! Ordered multi-value tag editing model.
//!
//! `TagSet` is a client-side editing representation that groups multi-value tags
//! for ergonomic editing and diffs against the original to produce mutations.
//! This is a UI concern — the server deals in raw `Vec<(String, String)>` pairs
//! and `Vec<TagOp>` mutations.

use std::collections::{HashMap, HashSet};

use mm_meta::mutations::TagOp;

// ============================================================================
// TagEntry
// ============================================================================

/// A single tag entry: one name with one or more values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagEntry {
    pub name: String,
    pub values: Vec<String>,
}

// ============================================================================
// TagSet
// ============================================================================

/// Ordered multi-value tag map.
///
/// Entries maintain insertion order for display stability. Each entry groups
/// all values for a given tag name (case-insensitive grouping, preserving
/// the original case of the first occurrence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TagSet {
    entries: Vec<TagEntry>,
}

impl TagSet {
    /// Group raw pairs by name, preserving first-seen order.
    ///
    /// Names are grouped case-insensitively (using UPPERCASE comparison),
    /// but the display name preserves the case of the first occurrence.
    pub fn from_pairs(pairs: Vec<(String, String)>) -> Self {
        // Track insertion order: normalized_name -> index in entries vec
        let mut index_map: HashMap<String, usize> = HashMap::new();
        let mut entries: Vec<TagEntry> = Vec::new();

        for (name, value) in pairs {
            let normalized = name.to_uppercase();
            if let Some(&idx) = index_map.get(&normalized) {
                entries[idx].values.push(value);
            } else {
                let idx = entries.len();
                index_map.insert(normalized, idx);
                entries.push(TagEntry {
                    name,
                    values: vec![value],
                });
            }
        }

        Self { entries }
    }

    /// Create an empty TagSet.
    pub fn empty() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Flatten back to pairs (for round-trip or display).
    pub fn to_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = Vec::new();
        for entry in &self.entries {
            for value in &entry.values {
                pairs.push((entry.name.clone(), value.clone()));
            }
        }
        pairs
    }

    /// Diff against original, produce minimal TagOps for an inode.
    ///
    /// Compares by normalized (UPPERCASE) tag name. For each name present in
    /// either set, computes added/removed values and emits the appropriate ops.
    pub fn diff(&self, original: &TagSet, inode: i64) -> Vec<TagOp> {
        let mut ops = Vec::new();

        // Build normalized name -> (display_name, values) for both
        let self_map = Self::normalized_map(&self.entries);
        let orig_map = Self::normalized_map(&original.entries);

        // All normalized names from both sides
        let mut all_names: HashSet<&str> = HashSet::new();
        for key in self_map.keys() {
            all_names.insert(key.as_str());
        }
        for key in orig_map.keys() {
            all_names.insert(key.as_str());
        }

        for normalized in all_names {
            let orig_values: HashSet<&str> = orig_map
                .get(normalized)
                .map(|(_, vals)| vals.iter().map(|s| s.as_str()).collect())
                .unwrap_or_default();

            let (display_name, curr_values_set) = match self_map.get(normalized) {
                Some((name, vals)) => (
                    name.as_str(),
                    vals.iter().map(|s| s.as_str()).collect::<HashSet<&str>>(),
                ),
                None => {
                    // Tag was removed entirely — use original's display name
                    let orig_name = orig_map
                        .get(normalized)
                        .map(|(n, _)| n.as_str())
                        .unwrap_or(normalized);
                    (orig_name, HashSet::new())
                }
            };

            // Removed values (in original but not in current)
            for &val in orig_values.difference(&curr_values_set) {
                ops.push(TagOp::drop_tag(inode, display_name, val));
            }

            // Added values (in current but not in original)
            for &val in curr_values_set.difference(&orig_values) {
                ops.push(TagOp::add_tag(inode, display_name, val));
            }
        }

        ops
    }

    /// Entry access (slice).
    pub fn entries(&self) -> &[TagEntry] {
        &self.entries
    }

    /// Number of entries (unique tag names).
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Get entry by index.
    pub fn get(&self, idx: usize) -> Option<&TagEntry> {
        self.entries.get(idx)
    }

    /// Get mutable entry by index.
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut TagEntry> {
        self.entries.get_mut(idx)
    }

    // ========================================================================
    // Editing operations
    // ========================================================================

    /// Add a new entry (tag name + initial value).
    ///
    /// If an entry with the same normalized name already exists, appends the
    /// value to that entry instead.
    pub fn add_entry(&mut self, name: String, value: String) {
        let normalized = name.to_uppercase();
        for entry in &mut self.entries {
            if entry.name.to_uppercase() == normalized {
                entry.values.push(value);
                return;
            }
        }
        self.entries.push(TagEntry {
            name,
            values: vec![value],
        });
    }

    /// Remove an entry by index.
    pub fn drop_entry(&mut self, idx: usize) {
        if idx < self.entries.len() {
            self.entries.remove(idx);
        }
    }

    /// Add a value to an existing entry.
    pub fn add_value(&mut self, entry_idx: usize, value: String) {
        if let Some(entry) = self.entries.get_mut(entry_idx) {
            entry.values.push(value);
        }
    }

    /// Remove a specific value from an entry.
    ///
    /// If this was the last value, the entry itself is removed.
    pub fn drop_value(&mut self, entry_idx: usize, value_idx: usize) {
        if let Some(entry) = self.entries.get_mut(entry_idx) {
            if value_idx < entry.values.len() {
                entry.values.remove(value_idx);
                if entry.values.is_empty() {
                    self.entries.remove(entry_idx);
                }
            }
        }
    }

    /// Replace a specific value in an entry.
    pub fn set_value(&mut self, entry_idx: usize, value_idx: usize, new: String) {
        if let Some(entry) = self.entries.get_mut(entry_idx) {
            if let Some(val) = entry.values.get_mut(value_idx) {
                *val = new;
            }
        }
    }

    /// Rename an entry (change the tag name).
    pub fn rename_entry(&mut self, entry_idx: usize, new_name: String) {
        if let Some(entry) = self.entries.get_mut(entry_idx) {
            entry.name = new_name;
        }
    }

    // ========================================================================
    // Internal helpers
    // ========================================================================

    /// Build normalized_name -> (display_name, all_values) map.
    fn normalized_map(entries: &[TagEntry]) -> HashMap<String, (String, Vec<String>)> {
        let mut map: HashMap<String, (String, Vec<String>)> = HashMap::new();
        for entry in entries {
            let normalized = entry.name.to_uppercase();
            let e = map
                .entry(normalized)
                .or_insert_with(|| (entry.name.clone(), Vec::new()));
            e.1.extend(entry.values.iter().cloned());
        }
        map
    }
}

// ============================================================================
// AggregatedTagSet
// ============================================================================

/// Aggregated tag state across multiple files for bulk editing.
#[derive(Debug, Clone)]
pub struct AggregatedTagSet {
    entries: Vec<AggregatedEntry>,
}

/// A single entry in an aggregated tag set.
#[derive(Debug, Clone)]
pub struct AggregatedEntry {
    pub name: String,
    pub state: AggregatedState,
}

/// The state of a tag across multiple files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AggregatedState {
    /// All files have identical value(s) for this tag.
    Consistent(Vec<String>),
    /// Files disagree on the value(s).
    Various,
    /// Operator set a new value overriding all files.
    Edited(Vec<String>),
}

impl AggregatedTagSet {
    /// Build an aggregated view from multiple per-file TagSets.
    ///
    /// For each tag name (case-insensitive), checks whether all files agree
    /// on the exact set of values.
    pub fn from_tag_sets(sets: &[TagSet]) -> Self {
        if sets.is_empty() {
            return Self {
                entries: Vec::new(),
            };
        }

        // Collect all normalized tag names in first-seen order
        let mut seen_order: Vec<String> = Vec::new(); // normalized names
        let mut display_names: HashMap<String, String> = HashMap::new(); // normalized -> display

        // For each normalized name, collect the value set per file
        let mut per_name: HashMap<String, Vec<Vec<String>>> = HashMap::new();

        for set in sets {
            // Track which names this file has
            let mut file_names: HashSet<String> = HashSet::new();

            for entry in &set.entries {
                let normalized = entry.name.to_uppercase();
                file_names.insert(normalized.clone());

                if !display_names.contains_key(&normalized) {
                    display_names.insert(normalized.clone(), entry.name.clone());
                    seen_order.push(normalized.clone());
                }

                let mut sorted_values = entry.values.clone();
                sorted_values.sort();
                per_name
                    .entry(normalized)
                    .or_default()
                    .push(sorted_values);
            }

            // For names not present in this file, push empty
            for normalized in &seen_order {
                if !file_names.contains(normalized) {
                    per_name.entry(normalized.clone()).or_default().push(Vec::new());
                }
            }
        }

        let num_files = sets.len();
        let mut entries = Vec::new();

        for normalized in &seen_order {
            let display_name = display_names
                .get(normalized)
                .cloned()
                .unwrap_or_else(|| normalized.clone());

            let file_values = per_name.get(normalized).unwrap();

            // Pad if some files were processed before this name was first seen
            let state = if file_values.len() == num_files
                && file_values.windows(2).all(|w| w[0] == w[1])
                && !file_values[0].is_empty()
            {
                AggregatedState::Consistent(file_values[0].clone())
            } else {
                AggregatedState::Various
            };

            entries.push(AggregatedEntry {
                name: display_name,
                state,
            });
        }

        Self { entries }
    }

    /// Entry access (slice).
    pub fn entries(&self) -> &[AggregatedEntry] {
        &self.entries
    }

    /// Number of entries.
    pub fn entry_count(&self) -> usize {
        self.entries.len()
    }

    /// Get entry by index.
    pub fn get(&self, idx: usize) -> Option<&AggregatedEntry> {
        self.entries.get(idx)
    }

    /// Get mutable entry by index.
    pub fn get_mut(&mut self, idx: usize) -> Option<&mut AggregatedEntry> {
        self.entries.get_mut(idx)
    }

    /// Apply edits back: for each Edited entry, produce TagOps that transform
    /// each file's original values to the edited ones.
    pub fn diff_all(
        &self,
        originals: &[TagSet],
        inodes: &[i64],
    ) -> Vec<TagOp> {
        assert_eq!(
            originals.len(),
            inodes.len(),
            "originals and inodes must have same length"
        );

        let mut ops = Vec::new();

        for entry in &self.entries {
            let new_values = match &entry.state {
                AggregatedState::Edited(vals) => vals,
                _ => continue, // Only edited entries produce ops
            };

            let normalized = entry.name.to_uppercase();

            for (file_idx, original) in originals.iter().enumerate() {
                let inode = inodes[file_idx];

                // Find original values for this tag name
                let orig_values: Vec<&str> = original
                    .entries()
                    .iter()
                    .filter(|e| e.name.to_uppercase() == normalized)
                    .flat_map(|e| e.values.iter().map(|s| s.as_str()))
                    .collect();

                let orig_set: HashSet<&str> = orig_values.iter().copied().collect();
                let new_set: HashSet<&str> =
                    new_values.iter().map(|s| s.as_str()).collect();

                // Drop values no longer present
                for &val in orig_set.difference(&new_set) {
                    ops.push(TagOp::drop_tag(inode, &entry.name, val));
                }

                // Add values not previously present
                for &val in new_set.difference(&orig_set) {
                    ops.push(TagOp::add_tag(inode, &entry.name, val));
                }
            }
        }

        ops
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_pairs_groups_by_name() {
        let pairs = vec![
            ("ARTIST".into(), "Bach".into()),
            ("TITLE".into(), "Fugue".into()),
            ("ARTIST".into(), "Handel".into()),
        ];
        let ts = TagSet::from_pairs(pairs);

        assert_eq!(ts.entry_count(), 2);
        assert_eq!(ts.entries()[0].name, "ARTIST");
        assert_eq!(ts.entries()[0].values, vec!["Bach", "Handel"]);
        assert_eq!(ts.entries()[1].name, "TITLE");
        assert_eq!(ts.entries()[1].values, vec!["Fugue"]);
    }

    #[test]
    fn from_pairs_case_insensitive_grouping() {
        let pairs = vec![
            ("Artist".into(), "Bach".into()),
            ("ARTIST".into(), "Handel".into()),
            ("artist".into(), "Vivaldi".into()),
        ];
        let ts = TagSet::from_pairs(pairs);

        assert_eq!(ts.entry_count(), 1);
        // First-seen case preserved
        assert_eq!(ts.entries()[0].name, "Artist");
        assert_eq!(
            ts.entries()[0].values,
            vec!["Bach", "Handel", "Vivaldi"]
        );
    }

    #[test]
    fn round_trip_to_pairs() {
        let pairs = vec![
            ("ARTIST".into(), "Bach".into()),
            ("TITLE".into(), "Fugue".into()),
            ("ARTIST".into(), "Handel".into()),
        ];
        let ts = TagSet::from_pairs(pairs);
        let out = ts.to_pairs();

        assert_eq!(
            out,
            vec![
                ("ARTIST".into(), "Bach".into()),
                ("ARTIST".into(), "Handel".into()),
                ("TITLE".into(), "Fugue".into()),
            ]
        );
    }

    #[test]
    fn diff_no_changes() {
        let pairs = vec![
            ("ARTIST".into(), "Bach".into()),
            ("TITLE".into(), "Fugue".into()),
        ];
        let original = TagSet::from_pairs(pairs.clone());
        let current = TagSet::from_pairs(pairs);

        let ops = current.diff(&original, 42);
        assert!(ops.is_empty());
    }

    #[test]
    fn diff_added_tag() {
        let original = TagSet::from_pairs(vec![("ARTIST".into(), "Bach".into())]);
        let current = TagSet::from_pairs(vec![
            ("ARTIST".into(), "Bach".into()),
            ("GENRE".into(), "Classical".into()),
        ]);

        let ops = current.diff(&original, 42);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].tag_name, "GENRE");
        assert_eq!(ops[0].old_value, None);
        assert_eq!(ops[0].new_value, Some("Classical".into()));
    }

    #[test]
    fn diff_removed_tag() {
        let original = TagSet::from_pairs(vec![
            ("ARTIST".into(), "Bach".into()),
            ("GENRE".into(), "Classical".into()),
        ]);
        let current = TagSet::from_pairs(vec![("ARTIST".into(), "Bach".into())]);

        let ops = current.diff(&original, 42);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].tag_name, "GENRE");
        assert_eq!(ops[0].old_value, Some("Classical".into()));
        assert_eq!(ops[0].new_value, None);
    }

    #[test]
    fn diff_multi_value_add_and_remove() {
        let original = TagSet::from_pairs(vec![
            ("GENRE".into(), "Classical".into()),
            ("GENRE".into(), "Baroque".into()),
        ]);
        let current = TagSet::from_pairs(vec![
            ("GENRE".into(), "Classical".into()),
            ("GENRE".into(), "Chamber".into()),
        ]);

        let mut ops = current.diff(&original, 42);
        ops.sort_by(|a, b| {
            a.old_value
                .as_deref()
                .unwrap_or("")
                .cmp(b.old_value.as_deref().unwrap_or(""))
        });

        assert_eq!(ops.len(), 2);
        // Drop "Baroque"
        assert!(ops.iter().any(|op| op.old_value == Some("Baroque".into())
            && op.new_value.is_none()));
        // Add "Chamber"
        assert!(ops.iter().any(|op| op.old_value.is_none()
            && op.new_value == Some("Chamber".into())));
    }

    #[test]
    fn editing_operations() {
        let mut ts = TagSet::from_pairs(vec![("ARTIST".into(), "Bach".into())]);

        // Add entry
        ts.add_entry("GENRE".into(), "Classical".into());
        assert_eq!(ts.entry_count(), 2);

        // Add value to existing entry (same normalized name)
        ts.add_entry("genre".into(), "Baroque".into());
        assert_eq!(ts.entry_count(), 2); // still 2 entries
        assert_eq!(ts.entries()[1].values.len(), 2);

        // Set value
        ts.set_value(0, 0, "Handel".into());
        assert_eq!(ts.entries()[0].values[0], "Handel");

        // Rename
        ts.rename_entry(0, "COMPOSER".into());
        assert_eq!(ts.entries()[0].name, "COMPOSER");

        // Drop value (leaves entry with one value)
        ts.drop_value(1, 0);
        assert_eq!(ts.entries()[1].values, vec!["Baroque"]);

        // Drop last value removes entry
        ts.drop_value(1, 0);
        assert_eq!(ts.entry_count(), 1);

        // Drop entry
        ts.drop_entry(0);
        assert_eq!(ts.entry_count(), 0);
    }

    #[test]
    fn aggregated_consistent() {
        let sets = vec![
            TagSet::from_pairs(vec![
                ("ARTIST".into(), "Bach".into()),
                ("ALBUM".into(), "WTC".into()),
            ]),
            TagSet::from_pairs(vec![
                ("ARTIST".into(), "Bach".into()),
                ("ALBUM".into(), "WTC".into()),
            ]),
        ];

        let agg = AggregatedTagSet::from_tag_sets(&sets);
        assert_eq!(agg.entry_count(), 2);
        assert_eq!(
            agg.entries()[0].state,
            AggregatedState::Consistent(vec!["Bach".into()])
        );
        assert_eq!(
            agg.entries()[1].state,
            AggregatedState::Consistent(vec!["WTC".into()])
        );
    }

    #[test]
    fn aggregated_various() {
        let sets = vec![
            TagSet::from_pairs(vec![("ARTIST".into(), "Bach".into())]),
            TagSet::from_pairs(vec![("ARTIST".into(), "Handel".into())]),
        ];

        let agg = AggregatedTagSet::from_tag_sets(&sets);
        assert_eq!(agg.entry_count(), 1);
        assert_eq!(agg.entries()[0].state, AggregatedState::Various);
    }

    #[test]
    fn aggregated_missing_tag_in_some_files() {
        let sets = vec![
            TagSet::from_pairs(vec![
                ("ARTIST".into(), "Bach".into()),
                ("GENRE".into(), "Classical".into()),
            ]),
            TagSet::from_pairs(vec![("ARTIST".into(), "Bach".into())]),
        ];

        let agg = AggregatedTagSet::from_tag_sets(&sets);
        assert_eq!(agg.entry_count(), 2);
        // ARTIST consistent
        assert_eq!(
            agg.entries()[0].state,
            AggregatedState::Consistent(vec!["Bach".into()])
        );
        // GENRE various (one file has it, one doesn't)
        assert_eq!(agg.entries()[1].state, AggregatedState::Various);
    }

    #[test]
    fn aggregated_diff_all() {
        let originals = vec![
            TagSet::from_pairs(vec![("ARTIST".into(), "Bach".into())]),
            TagSet::from_pairs(vec![("ARTIST".into(), "Handel".into())]),
        ];
        let inodes = vec![100, 200];

        let mut agg = AggregatedTagSet::from_tag_sets(&originals);
        // Edit ARTIST to "Vivaldi" for all
        agg.entries.get_mut(0).unwrap().state =
            AggregatedState::Edited(vec!["Vivaldi".into()]);

        let mut ops = agg.diff_all(&originals, &inodes);
        ops.sort_by_key(|op| op.inode);

        assert_eq!(ops.len(), 4); // 2 drops + 2 adds
        // File 100: drop "Bach", add "Vivaldi"
        assert!(ops
            .iter()
            .any(|op| op.inode == 100
                && op.old_value == Some("Bach".into())
                && op.new_value.is_none()));
        assert!(ops
            .iter()
            .any(|op| op.inode == 100
                && op.old_value.is_none()
                && op.new_value == Some("Vivaldi".into())));
        // File 200: drop "Handel", add "Vivaldi"
        assert!(ops
            .iter()
            .any(|op| op.inode == 200
                && op.old_value == Some("Handel".into())
                && op.new_value.is_none()));
        assert!(ops
            .iter()
            .any(|op| op.inode == 200
                && op.old_value.is_none()
                && op.new_value == Some("Vivaldi".into())));
    }
}
