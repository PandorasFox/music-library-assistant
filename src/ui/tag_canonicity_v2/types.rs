//! Tag Canonicity V2 Types
//!
//! Data structures for the three-pane tag canonicity resolution modal, including
//! file tag info, selection state, and mutation generation.
//!
//! Key behaviors:
//! - Two focusable panes: variants (left) and files (middle)
//! - Right pane shows tag values for selected file (informational only)
//! - Spacebar toggle-select for each variant
//! - F key fills text input from hovered variant
//! - Editable text field for canonical value

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::corpus::db::types::FileSource;
use crate::meta::signals::data::{
    InconsistentAlbumArtistSignal, TagCanonicitySignal,
};
use crate::corpus::db::ReadOnlyDb;
use crate::meta::mutations::{Mutation, TagOp};
use crate::meta::mutations::tag_edit::ApplyTagOpsMutation;
use crate::corpus::paths;
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

/// File info with cached tag values for display.
#[derive(Debug, Clone)]
pub struct FileTagInfo {
    /// Inode of the file
    pub inode: i64,
    /// Display name (basename)
    pub filename: String,
    /// Full corpus-relative path
    pub path: String,
    /// All tags for this file (tag_name, tag_value)
    pub tag_values: Vec<(String, String)>,
}

/// Extended modal data with per-file tag info.
#[derive(Debug, Clone)]
pub struct TagCanonicalityModalDataV2 {
    /// The tag name being canonicalized (e.g., "artist", "album_artist")
    pub tag_name: String,
    /// Optional context label (e.g., "Album: Clockwork Hearts")
    pub context_label: Option<String>,
    /// Variants sorted by count DESC, then alphabetically for ties
    pub variants: Vec<TagVariantEntry>,
    /// Inodes affected by this canonicalization
    pub inodes: Vec<i64>,
    /// Per-file tag info with cached tag values
    pub files: Vec<FileTagInfo>,
}

impl TagCanonicalityModalDataV2 {
    /// Create from a typed `TagCanonicitySignal`, loading file info from database.
    pub fn from_tag_canonicity(signal: &TagCanonicitySignal, read_db: &ReadOnlyDb) -> Option<Self> {
        let tag_name = signal.tag_name.clone();

        let mut variants: Vec<TagVariantEntry> = signal
            .data
            .variants
            .iter()
            .map(|(value, count)| TagVariantEntry {
                value: value.clone(),
                count: *count,
            })
            .collect();

        variants.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));

        let inodes = signal.data.inodes.clone();
        let files = Self::load_file_info(&inodes, read_db);

        Some(Self {
            tag_name,
            context_label: None,
            variants,
            inodes,
            files,
        })
    }

    /// Create from a typed `InconsistentAlbumArtistSignal`, loading file info from database.
    pub fn from_inconsistent_album_artist(
        signal: &InconsistentAlbumArtistSignal,
        read_db: &ReadOnlyDb,
    ) -> Option<Self> {
        let tag_name = "album_artist".to_string();
        let context_label = Some(signal.data.album.clone());

        let mut variants: Vec<TagVariantEntry> = signal
            .data
            .album_artist_variants
            .iter()
            .map(|(value, count)| TagVariantEntry {
                value: value.clone(),
                count: *count,
            })
            .collect();

        variants.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));

        let inodes = signal.data.inodes.clone();
        let files = Self::load_file_info(&inodes, read_db);

        Some(Self {
            tag_name,
            context_label,
            variants,
            inodes,
            files,
        })
    }

    /// Load file info (filename, path, tags) for a set of inodes.
    fn load_file_info(inodes: &[i64], read_db: &ReadOnlyDb) -> Vec<FileTagInfo> {
        let resolver = paths::get_resolver();
        let mut files = Vec::new();

        for &inode in inodes {
            if let Ok(Some(audio_file)) =
                read_db.get_audio_file_by_inode(inode, FileSource::Corpus)
            {
                let path = audio_file.path();
                let filename = Path::new(path)
                    .file_name()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string());

                // Load tags from disk
                let abs_path = resolver.resolve(Path::new(path));
                let tagset = TagSet::from_file(&abs_path).unwrap_or_else(|_| TagSet::empty());

                // Get all tags as (name, value) pairs
                let tag_values: Vec<(String, String)> = tagset
                    .iter()
                    .map(|(k, v)| (k.to_string(), v.to_string()))
                    .collect();

                files.push(FileTagInfo {
                    inode,
                    filename,
                    path: path.to_string(),
                    tag_values,
                });
            }
        }

        // Sort files alphabetically by filename for consistent display
        files.sort_by(|a, b| a.filename.cmp(&b.filename));
        files
    }

    /// Get the default canonical value for pre-filling.
    ///
    /// Returns the most common value. For ties, uses alphabetical order.
    /// Returns empty string if no variants.
    pub fn default_canonical(&self) -> String {
        self.variants
            .first()
            .map(|v| v.value.clone())
            .unwrap_or_default()
    }
}

/// Which pane has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPaneV2 {
    /// Left pane: variant values
    #[default]
    Variants,
    /// Middle pane: file list
    Files,
}

/// State for the three-pane tag canonicity modal.
#[derive(Debug, Clone)]
pub struct TagCanonicalityStateV2 {
    /// Loaded data (immutable during interaction)
    pub data: TagCanonicalityModalDataV2,
    /// Which variants are selected for squashing (indices into data.variants)
    pub selected_variants: HashSet<usize>,
    /// Currently highlighted position in variants pane:
    /// - -1 = text field
    /// - 0..n = variant list indices
    pub variant_cursor: i32,
    /// Currently highlighted file in files pane
    pub file_cursor: usize,
    /// Which pane has focus
    pub focus_pane: FocusPaneV2,
    /// Canonical value text input (with cursor, Ctrl+U/K/A/E support)
    pub canonical_input: TextInputState,
    /// Whether the canonical value was pre-filled
    pub pre_filled: bool,
    /// Current group index (0-based)
    pub group_index: usize,
    /// Total number of groups
    pub total_groups: usize,
    /// Scroll offset for variants list
    pub variant_scroll: usize,
    /// Scroll offset for files list
    pub file_scroll: usize,
}

impl TagCanonicalityStateV2 {
    /// Path of the currently selected file (for status bar).
    pub fn selected_path(&self) -> Option<&str> {
        self.data.files.get(self.file_cursor).map(|f| f.path.as_str())
    }

    /// Create a new state from data.
    ///
    /// If `pre_fill` is true, pre-fills the canonical input with the most common value.
    /// For album_artist resolution, `pre_fill` should be false.
    ///
    /// `group_index` and `total_groups` are used to display "N of M" in the title.
    pub fn new(
        data: TagCanonicalityModalDataV2,
        pre_fill: bool,
        group_index: usize,
        total_groups: usize,
    ) -> Self {
        let canonical_value = if pre_fill {
            data.default_canonical()
        } else {
            String::new()
        };

        // ALL items selected initially
        let selected_variants: HashSet<usize> = (0..data.variants.len()).collect();

        // Create text input state with cursor at end
        let mut canonical_input = TextInputState::new();
        canonical_input.set_value(canonical_value);
        canonical_input.focused = false; // Not focused by default

        Self {
            data,
            selected_variants,
            variant_cursor: 0, // Default focus = first list item (not text field)
            file_cursor: 0,
            focus_pane: FocusPaneV2::Variants,
            canonical_input,
            pre_filled: pre_fill,
            group_index,
            total_groups,
            variant_scroll: 0,
            file_scroll: 0,
        }
    }

    /// Restore UI state from a previously staged decision's mutations.
    ///
    /// When navigating back to a cluster that already has a staged decision,
    /// this method extracts the canonical value and selected variants from
    /// the stored mutations and applies them to the modal state.
    pub fn restore_from_mutations(&mut self, mutations: &[Mutation]) {
        // Find ApplyTagOps mutation and extract tag operations
        let ops: Vec<&TagOp> = mutations
            .iter()
            .filter_map(|m| match m {
                Mutation::ApplyTagOps(ref m) => Some(m.ops.iter()),
                _ => None,
            })
            .flatten()
            .collect();

        if ops.is_empty() {
            return;
        }

        // Extract the canonical value (new_value from any op that has one)
        let canonical_value = ops
            .iter()
            .find_map(|op| op.new_value.as_ref())
            .cloned()
            .unwrap_or_default();

        // Extract the variants that were selected (old_values from the ops)
        let selected_old_values: HashSet<String> = ops
            .iter()
            .filter_map(|op| op.old_value.as_ref().cloned())
            .collect();

        // Map old_values back to variant indices
        let mut selected_variants: HashSet<usize> = HashSet::new();
        for (idx, variant) in self.data.variants.iter().enumerate() {
            if selected_old_values.contains(&variant.value) {
                selected_variants.insert(idx);
            }
        }

        // Apply restored state
        self.canonical_input.set_value(canonical_value);
        self.selected_variants = selected_variants;
        self.pre_filled = true; // Already has a confirmed value
    }

    /// Toggle selection of the variant at cursor (only works if variant_cursor >= 0).
    pub fn toggle_selection(&mut self) {
        if self.variant_cursor >= 0 {
            let idx = self.variant_cursor as usize;
            if idx < self.data.variants.len() {
                if self.selected_variants.contains(&idx) {
                    self.selected_variants.remove(&idx);
                } else {
                    self.selected_variants.insert(idx);
                }
            }
        }
    }

    /// Move variant cursor up (including into text field at -1).
    pub fn variant_cursor_up(&mut self) {
        if self.variant_cursor > -1 {
            self.variant_cursor -= 1;
            // Update text input focus
            self.canonical_input.focused = self.variant_cursor == -1;
        }
    }

    /// Move variant cursor down (from text field into list, or within list).
    pub fn variant_cursor_down(&mut self) {
        let max_cursor = self.data.variants.len() as i32 - 1;
        if self.variant_cursor < max_cursor {
            self.variant_cursor += 1;
            self.canonical_input.focused = false;
        }
    }

    /// Move file cursor up.
    pub fn file_cursor_up(&mut self) {
        if self.file_cursor > 0 {
            self.file_cursor -= 1;
        }
    }

    /// Move file cursor down.
    pub fn file_cursor_down(&mut self) {
        if self.file_cursor + 1 < self.data.files.len() {
            self.file_cursor += 1;
        }
    }

    /// Check if we can submit (canonical value is non-empty and variants selected).
    pub fn can_submit(&self) -> bool {
        !self.canonical_input.value().trim().is_empty() && !self.selected_variants.is_empty()
    }

    /// Get the selected variant values that should be replaced.
    pub fn selected_variant_values(&self) -> Vec<&str> {
        self.selected_variants
            .iter()
            .filter_map(|&idx| self.data.variants.get(idx))
            .map(|v| v.value.as_str())
            .collect()
    }

    /// Generate mutations for the selected variants -> canonical value using incremental TagOps.
    ///
    /// Uses the cached file data from modal initialization.
    /// Returns a single ApplyTagOps mutation containing ops for all affected tracks.
    pub fn mutations(&self) -> Vec<Mutation> {
        let canonical = self.canonical_input.value().trim();
        if canonical.is_empty() {
            return Vec::new();
        }

        let selected_variants: HashSet<&str> = self.selected_variant_values().into_iter().collect();
        if selected_variants.is_empty() {
            return Vec::new();
        }

        // Build track_info from cached file data
        let resolver = paths::get_resolver();
        let track_info: HashMap<i64, (PathBuf, TagSet)> = self
            .data
            .files
            .iter()
            .filter_map(|f| {
                let abs_path = resolver.resolve(Path::new(&f.path));
                let tagset = TagSet::from_file(&abs_path).ok()?;
                Some((f.inode, (abs_path, tagset)))
            })
            .collect();

        let mut ops = Vec::new();

        // For each file, generate TagOps if its current value is a selected variant
        for &inode in &self.data.inodes {
            if let Some((_path, current_tagset)) = track_info.get(&inode) {
                // Get current values for this tag from the TagSet
                let current_values: Vec<&str> =
                    current_tagset.values_for(&self.data.tag_name).collect();

                // Special case: if tag is MISSING (current_values empty) and "" is selected,
                // treat this as "missing tag needs to be set to canonical".
                let tag_is_missing = current_values.is_empty();
                let missing_is_selected = selected_variants.contains("");

                // Find which existing values match selected variants
                let matching_variants: Vec<&str> = current_values
                    .iter()
                    .filter(|v| selected_variants.contains(*v) && **v != canonical)
                    .copied()
                    .collect();

                // Skip if no matching variants AND we're not handling a missing tag
                if matching_variants.is_empty() && !(tag_is_missing && missing_is_selected) {
                    continue;
                }

                // Generate TagOps for this inode:
                // - Drop each matching variant
                // - Add the canonical value (only once, and only if not already present)
                //
                // Edge case: if canonical already exists on the file (e.g., file has both
                // "RIOT" and "RIOT "), we just drop the non-canonical variants.
                let canonical_already_exists = current_values.iter().any(|&v| v == canonical);
                let mut added_canonical = canonical_already_exists;

                for variant in &matching_variants {
                    if !added_canonical {
                        // Replace: drop old variant, add canonical
                        ops.push(TagOp::replace_tag(
                            inode,
                            &self.data.tag_name,
                            *variant,
                            canonical,
                        ));
                        added_canonical = true;
                    } else {
                        // Already added canonical (or it already exists), just drop this variant
                        ops.push(TagOp::drop_tag(inode, &self.data.tag_name, *variant));
                    }
                }

                // If tag was missing and "" was selected, add the canonical value
                if tag_is_missing && missing_is_selected && !added_canonical {
                    ops.push(TagOp::add_tag(inode, &self.data.tag_name, canonical));
                }
            }
        }

        if ops.is_empty() {
            Vec::new()
        } else {
            vec![Mutation::ApplyTagOps(ApplyTagOpsMutation { ops })]
        }
    }
}

/// Action returned from handling input in the modal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagCanonicalityActionV2 {
    /// No action, continue showing modal
    None,
    /// User confirmed current squash (Enter) - stage decision and advance
    Confirmed,
    /// User cancelled entire modal (Esc)
    Cancelled,
    /// User navigated to next/prev cluster (Tab/Shift-Tab) - does NOT stage decision
    Navigate {
        /// True = forward (Tab), false = backward (Shift-Tab)
        forward: bool,
    },
    /// User requested review screen (Ctrl+R)
    ShowReview,
}
