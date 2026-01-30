//! Tag Canonicity Modal Types
//!
//! Data structures for the tag canonicity resolution modal, including
//! variant data, selection state, and mutation generation.
//!
//! Key behaviors:
//! - Spacebar toggle-select for each variant
//! - Editable text field for canonical value
//! - Pre-filled to most common value (alphabetical tiebreaker) for tag canonicity
//! - NOT pre-filled for album_artist resolution

use std::collections::HashSet;

use std::path::PathBuf;

use crate::corpus::db::types::AggregateSignal;
use crate::corpus::health::album_artist_detection::AlbumArtistIssue;
use crate::corpus::health::collision::TagCollision;
use crate::corpus::mutations::Mutation;
use crate::corpus::tags::TagSet;
use crate::ui::widgets::TextInputState;

/// A tag variant with its occurrence count.
#[derive(Debug, Clone)]
pub struct TagVariantEntry {
    /// The tag value
    pub value: String,
    /// Number of tracks with this value
    pub count: usize,
}

/// Data loaded once when the modal opens.
///
/// This is immutable during modal interaction - all DB queries happen at load time.
#[derive(Debug, Clone)]
pub struct TagCanonicalityModalData {
    /// The tag name being canonicalized (e.g., "artist", "album_artist")
    pub tag_name: String,
    /// Optional context label (e.g., "Album: Clockwork Hearts")
    pub context_label: Option<String>,
    /// Variants sorted by count DESC, then alphabetically for ties
    pub variants: Vec<TagVariantEntry>,
    /// Track IDs affected by this canonicalization
    pub track_ids: Vec<i64>,
}

impl TagCanonicalityModalData {
    /// Create from a TagCollision (artist, genre canonicity).
    ///
    /// Note: track_ids are not directly available from TagCollision.
    /// They must be populated separately via a database query if needed for mutations.
    pub fn from_collision(collision: &TagCollision) -> Self {
        // Build variants from the collision's variant_counts
        let mut variants: Vec<TagVariantEntry> = collision
            .variant_counts
            .iter()
            .map(|(value, count)| TagVariantEntry {
                value: value.clone(),
                count: *count,
            })
            .collect();

        // Sort: count DESC, then alphabetically for ties
        variants.sort_by(|a, b| {
            b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value))
        });

        Self {
            tag_name: collision.tag_name.clone(),
            context_label: None,
            variants,
            // Track IDs need to be populated separately via DB query
            track_ids: Vec::new(),
        }
    }

    /// Create from an AlbumArtistIssue (album_artist resolution).
    pub fn from_album_artist_issue(issue: &AlbumArtistIssue) -> Self {
        // For album_artist, we show the artist variants as the list
        // The user will input the album_artist value
        let mut variants: Vec<TagVariantEntry> = issue
            .artist_variants
            .iter()
            .map(|(value, count)| TagVariantEntry {
                value: value.clone(),
                count: *count,
            })
            .collect();

        // Sort: count DESC, then alphabetically for ties
        variants.sort_by(|a, b| {
            b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value))
        });

        Self {
            tag_name: "album_artist".to_string(),
            context_label: Some(format!("Album: {}", issue.album)),
            variants,
            track_ids: issue.track_ids.clone(),
        }
    }

    /// Create from an AggregateSignal (loaded from database).
    ///
    /// Handles two signal formats:
    /// - TagCanonicity: { tag_name, variants: {value: count}, track_ids, context? }
    /// - InconsistentAlbumArtist: { album, album_artist_variants: {value: count}, track_ids }
    pub fn from_signal(signal: &AggregateSignal) -> Option<Self> {
        use crate::corpus::db::types::AggregateSignalType;

        let metadata = signal.metadata_json.as_ref()?;
        let json: serde_json::Value = serde_json::from_str(metadata).ok()?;

        // Handle InconsistentAlbumArtist format
        if signal.signal_type == AggregateSignalType::InconsistentAlbumArtist {
            let variants_obj = json.get("album_artist_variants")?.as_object()?;
            let mut variants: Vec<TagVariantEntry> = variants_obj
                .iter()
                .filter_map(|(value, count)| {
                    Some(TagVariantEntry {
                        value: value.clone(),
                        count: count.as_u64()? as usize,
                    })
                })
                .collect();

            variants.sort_by(|a, b| {
                b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value))
            });

            let track_ids = json
                .get("track_ids")
                .and_then(|v| v.as_array())
                .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
                .unwrap_or_default();

            let album = json.get("album").and_then(|v| v.as_str()).map(String::from);

            return Some(Self {
                tag_name: "album_artist".to_string(),
                context_label: album,
                variants,
                track_ids,
            });
        }

        // Standard TagCanonicity format
        let tag_name = json.get("tag_name")?.as_str()?.to_string();

        let variants_obj = json.get("variants")?.as_object()?;
        let mut variants: Vec<TagVariantEntry> = variants_obj
            .iter()
            .filter_map(|(value, count)| {
                Some(TagVariantEntry {
                    value: value.clone(),
                    count: count.as_u64()? as usize,
                })
            })
            .collect();

        variants.sort_by(|a, b| {
            b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value))
        });

        let track_ids = json
            .get("track_ids")
            .and_then(|v| v.as_array())
            .map(|arr| arr.iter().filter_map(|v| v.as_i64()).collect())
            .unwrap_or_default();

        Some(Self {
            tag_name,
            context_label: json.get("context").and_then(|v| v.as_str()).map(String::from),
            variants,
            track_ids,
        })
    }

    /// Get the default canonical value for pre-filling.
    ///
    /// Returns the most common value. For ties, uses alphabetical order.
    /// Returns empty string if no variants.
    pub fn default_canonical(&self) -> String {
        self.variants.first().map(|v| v.value.clone()).unwrap_or_default()
    }
}

/// Modal overlay state for confirmation dialogs.
///
/// Currently unused - navigation is non-committal so no confirmation needed.
/// Retained for future use (e.g., confirming destructive actions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagCanonicalityModal {
    /// No modal overlay
    #[default]
    None,
}

/// State for the tag canonicity resolution modal.
///
/// Layout: text field at top (cursor = -1), variant list below (cursor >= 0).
/// Default focus is first list item (cursor = 0).
#[derive(Debug, Clone)]
pub struct TagCanonicalityState {
    /// Loaded data (immutable during interaction)
    pub data: TagCanonicalityModalData,
    /// Which variants are selected for squashing (indices into data.variants)
    pub selected: HashSet<usize>,
    /// Currently highlighted position:
    /// - -1 = text field at top
    /// - 0..n = variant list indices
    pub cursor: i32,
    /// Canonical value text input (with cursor, Ctrl+U/K/A/E support)
    pub canonical_input: TextInputState,
    /// Whether the canonical value was pre-filled
    pub pre_filled: bool,
    /// Modal overlay (confirmation dialogs) - currently unused
    pub modal: TagCanonicalityModal,
    /// Current group index (0-based)
    pub group_index: usize,
    /// Total number of groups
    pub total_groups: usize,
}

impl TagCanonicalityState {
    /// Create a new state from data.
    ///
    /// If `pre_fill` is true, pre-fills the canonical input with the most common value.
    /// For album_artist resolution, `pre_fill` should be false.
    ///
    /// Layout: text field at top (cursor=-1), variant list below (cursor>=0).
    /// Default focus: first list item (cursor=0).
    /// All items selected initially.
    ///
    /// `group_index` and `total_groups` are used to display "N of M" in the modal title.
    pub fn new(data: TagCanonicalityModalData, pre_fill: bool, group_index: usize, total_groups: usize) -> Self {
        let canonical_value = if pre_fill {
            data.default_canonical()
        } else {
            String::new()
        };

        // ALL items selected initially
        let selected: HashSet<usize> = (0..data.variants.len()).collect();

        // Create text input state with cursor at end
        let mut canonical_input = TextInputState::new();
        canonical_input.set_value(canonical_value);
        canonical_input.focused = false; // Not focused by default

        Self {
            data,
            selected,
            cursor: 0, // Default focus = first list item (not text field)
            canonical_input,
            pre_filled: pre_fill,
            modal: TagCanonicalityModal::None,
            group_index,
            total_groups,
        }
    }

    /// Toggle selection of the variant at cursor (only works if cursor >= 0).
    pub fn toggle_selection(&mut self) {
        if self.cursor >= 0 {
            let idx = self.cursor as usize;
            if idx < self.data.variants.len() {
                if self.selected.contains(&idx) {
                    self.selected.remove(&idx);
                } else {
                    self.selected.insert(idx);
                }
            }
        }
    }

    /// Move cursor up (including into text field at -1).
    pub fn cursor_up(&mut self) {
        if self.cursor > -1 {
            self.cursor -= 1;
            // Update text input focus
            self.canonical_input.focused = self.cursor == -1;
        }
    }

    /// Move cursor down (from text field into list, or within list).
    pub fn cursor_down(&mut self) {
        let max_cursor = self.data.variants.len() as i32 - 1;
        if self.cursor < max_cursor {
            self.cursor += 1;
            self.canonical_input.focused = false;
        }
    }

    /// Check if cursor is on text field.
    pub fn is_on_text_field(&self) -> bool {
        self.cursor == -1
    }

    /// Check if cursor is on first list item.
    pub fn is_on_first_item(&self) -> bool {
        self.cursor == 0
    }

    /// Check if cursor is on last list item.
    pub fn is_on_last_item(&self) -> bool {
        self.cursor == self.data.variants.len() as i32 - 1
    }

    /// Check if we can submit (canonical value is non-empty and variants selected, no modal open).
    pub fn can_submit(&self) -> bool {
        self.modal == TagCanonicalityModal::None
            && !self.canonical_input.value().trim().is_empty()
            && !self.selected.is_empty()
    }

    /// Close any open modal.
    pub fn close_modal(&mut self) {
        self.modal = TagCanonicalityModal::None;
    }

    /// Check if a modal is open.
    pub fn has_modal(&self) -> bool {
        self.modal != TagCanonicalityModal::None
    }

    /// Get the canonical value to apply.
    pub fn canonical_value(&self) -> &str {
        self.canonical_input.value().trim()
    }

    /// Get the selected variant values that should be replaced.
    pub fn selected_variants(&self) -> Vec<&str> {
        self.selected
            .iter()
            .filter_map(|&idx| self.data.variants.get(idx))
            .map(|v| v.value.as_str())
            .collect()
    }

    /// Generate mutations for the selected variants → canonical value using DB-first pattern.
    ///
    /// Requires track_info: a map from track_id to (path, current_tagset).
    /// Each track's current tag value is checked against selected variants.
    /// Only tracks whose current value is a selected non-canonical variant get edits.
    pub fn mutations_with_paths(
        &self,
        track_info: &std::collections::HashMap<i64, (PathBuf, TagSet)>,
    ) -> Vec<Mutation> {
        let canonical = self.canonical_input.value().trim();
        if canonical.is_empty() {
            return Vec::new();
        }

        let selected_variants: HashSet<&str> = self.selected_variants().into_iter().collect();
        if selected_variants.is_empty() {
            return Vec::new();
        }

        let mut mutations = Vec::new();

        // For each track, create a mutation only if its current value is a selected variant
        for &track_id in &self.data.track_ids {
            if let Some((path, current_tagset)) = track_info.get(&track_id) {
                // Get current values for this tag from the TagSet
                let current_values: Vec<&str> = current_tagset
                    .values_for(&self.data.tag_name)
                    .collect();

                // Find which values match selected variants
                let matching_variants: Vec<String> = current_values
                    .iter()
                    .filter(|v| selected_variants.contains(*v) && **v != canonical)
                    .map(|v| v.to_string())
                    .collect();

                if matching_variants.is_empty() {
                    continue; // No matching variants to replace
                }

                // Build new TagSet: replace matching variants with canonical value
                let mut new_tags: Vec<(String, String)> = Vec::new();
                let mut added_canonical = false;

                for (k, v) in current_tagset.iter() {
                    if k.eq_ignore_ascii_case(&self.data.tag_name) {
                        // This is the tag we're canonicalizing
                        if matching_variants.iter().any(|mv| mv == v) {
                            // Replace with canonical (but only add once)
                            if !added_canonical {
                                new_tags.push((k.to_string(), canonical.to_string()));
                                added_canonical = true;
                            }
                        } else {
                            // Keep other values for this tag unchanged
                            new_tags.push((k.to_string(), v.to_string()));
                        }
                    } else {
                        // Keep other tags unchanged
                        new_tags.push((k.to_string(), v.to_string()));
                    }
                }

                // If we replaced variants but didn't add canonical yet (shouldn't happen), add it
                if !matching_variants.is_empty() && !added_canonical {
                    new_tags.push((self.data.tag_name.clone(), canonical.to_string()));
                }

                // DB-first pattern: SetTrackTagsDb then FlushTagsToDisk
                mutations.push(Mutation::SetTrackTagsDb {
                    track_id,
                    tags: new_tags,
                });
                mutations.push(Mutation::FlushTagsToDisk {
                    track_id,
                    path: path.clone(),
                });
            }
        }

        mutations
    }
}

/// Action returned from handling input in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagCanonicalityAction {
    /// No action, continue showing modal
    None,
    /// User confirmed current squash (Enter) - stage decision and advance
    Confirmed,
    /// User cancelled entire flow (Esc from main, not from modal)
    Cancelled,
    /// User navigated to next/prev cluster (Tab/Shift-Tab) - does NOT stage decision
    Navigate {
        /// True = forward (Tab), false = backward (Shift-Tab)
        forward: bool,
    },
    /// User requested review screen (Ctrl+R)
    ShowReview,
}
