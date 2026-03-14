//! Tree Browser Actions
//!
//! Actions returned from tree browser key handling. The browser returns these;
//! the caller (App) decides what to do with them.

/// Actions returned from tree browser input handling.
///
/// CorpusBrowser keeps CycleNext/CyclePrev/Cancel as domain actions because
/// it captures Tab when config panel/search/filter is active, and Cancel
/// has multi-modal behavior (close panel, return to health).
///
/// Path strings are relative to the archive root (same as BrowserEntry.path).
#[derive(Debug, Clone)]
pub enum TreeBrowserAction {
    /// User pressed Esc - cancel and return to previous mode
    Cancel,

    /// Cycle to next view in lateral ring (Tab)
    CycleNext,

    /// Cycle to previous view in lateral ring (Shift-Tab)
    CyclePrev,

    /// Edit all tracks in directory subtree (Enter on directory)
    EditDirectory(String),

    /// Edit single file (Enter on file)
    EditFile(String),

    /// Open directory config panel (C on source root)
    OpenDirConfig(String),

    /// Save dir config edits (Enter on Save button)
    SaveDirConfig,

    /// Close dir config panel (Esc or Discard)
    CloseDirConfig,

    /// Review pending transaction (R key with pending dir config edits)
    ReviewTransaction,
}
