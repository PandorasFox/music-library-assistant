//! DirConfigEditorState — shared state for editing a source directory's config.
//!
//! Used by both TUI (as overlay/panel state) and web (form population).
//! Tracks pending field changes; "Confirm" builds a mutation for staging
//! into the transaction system.

use std::path::PathBuf;

use mm_meta::config::SourceDir;

/// Editing state for a single source directory's config.
///
/// Created from a `GetDirConfig` response. Fields are the editable copies;
/// `original` holds the server state for dirty checking and mutation building.
#[derive(Debug, Clone)]
pub struct DirConfigEditorState {
    /// Which directory (corpus-relative path).
    pub source_path: PathBuf,
    /// Original source dir from server (None = no explicit config exists).
    pub original: Option<SourceDir>,
    /// Editable fields (None = inherit from parent):
    pub libraries: Vec<String>,
    pub can_stash_dupes: Option<bool>,
    pub interior_dupes: Option<bool>,
    pub path_schema: Option<String>,
    pub enable_acoustid: Option<bool>,
    pub pinned_release: Option<String>,
}

impl DirConfigEditorState {
    /// Create editor state from a `GetDirConfig` response.
    pub fn new(source_path: PathBuf, source_dir: Option<SourceDir>) -> Self {
        let (libraries, can_stash_dupes, interior_dupes, path_schema, enable_acoustid, pinned_release) =
            match &source_dir {
                Some(sd) => (
                    sd.libraries.clone(),
                    sd.can_stash_dupes,
                    sd.interior_dupes,
                    sd.path_schema.as_ref().map(|s| s.template.clone()),
                    sd.enable_acoustid,
                    sd.pinned_release.clone(),
                ),
                None => (vec![], None, None, None, None, None),
            };
        Self {
            source_path,
            original: source_dir,
            libraries,
            can_stash_dupes,
            interior_dupes,
            path_schema,
            enable_acoustid,
            pinned_release,
        }
    }

    /// Whether any field has been changed from the original.
    pub fn has_changes(&self) -> bool {
        let (orig_libs, orig_csd, orig_id, orig_ps, orig_ea, orig_pr) = match &self.original {
            Some(sd) => (
                &sd.libraries,
                sd.can_stash_dupes,
                sd.interior_dupes,
                sd.path_schema.as_ref().map(|s| s.template.clone()),
                sd.enable_acoustid,
                sd.pinned_release.clone(),
            ),
            None => {
                // Any non-default value is a change from "no config"
                return !self.libraries.is_empty()
                    || self.can_stash_dupes.is_some()
                    || self.interior_dupes.is_some()
                    || self.path_schema.is_some()
                    || self.enable_acoustid.is_some()
                    || self.pinned_release.is_some();
            }
        };
        self.libraries != *orig_libs
            || self.can_stash_dupes != orig_csd
            || self.interior_dupes != orig_id
            || self.path_schema != orig_ps
            || self.enable_acoustid != orig_ea
            || self.pinned_release != orig_pr
    }

    /// Build a `SourceDir` from the current edited values.
    pub fn to_source_dir(&self) -> SourceDir {
        SourceDir {
            path: self.source_path.clone(),
            libraries: self.libraries.clone(),
            can_stash_dupes: self.can_stash_dupes,
            interior_dupes: self.interior_dupes,
            path_schema: self
                .path_schema
                .as_ref()
                .and_then(|t| mm_meta::config::path_schema::parse_path_schema(t).ok()),
            enable_acoustid: self.enable_acoustid,
            pinned_release: self.pinned_release.clone(),
        }
    }

    /// Build the original `SourceDir` (for the mutation's `old_dir` field).
    pub fn original_source_dir(&self) -> SourceDir {
        match &self.original {
            Some(sd) => sd.clone(),
            None => SourceDir {
                path: self.source_path.clone(),
                libraries: vec![],
                can_stash_dupes: None,
                interior_dupes: None,
                path_schema: None,
                enable_acoustid: None,
                pinned_release: None,
            },
        }
    }

    /// Cycle an `Option<bool>` through None → Some(true) → Some(false) → None.
    pub fn cycle_opt_bool(v: Option<bool>) -> Option<bool> {
        match v {
            None => Some(true),
            Some(true) => Some(false),
            Some(false) => None,
        }
    }
}
