//! Tree Browser Actions
//!
//! Actions returned from tree browser key handling. The browser returns these;
//! the caller (App) decides what to do with them.

use std::path::PathBuf;

/// Actions returned from tree browser input handling.
#[derive(Debug, Clone)]
pub enum TreeBrowserAction {
    /// No action needed, stay in browser
    None,

    /// User pressed Esc - cancel and return to previous mode
    Cancel,

    /// Cycle to next view in lateral ring (Tab)
    CycleNext,

    /// Cycle to previous view in lateral ring (Shift-Tab)
    CyclePrev,

    /// Edit all tracks in directory subtree (Enter on directory)
    EditDirectory(PathBuf),

    /// Edit single file (Enter on file)
    EditFile(PathBuf),

    /// Open filter popup (Ctrl+/)
    OpenFilter,

    /// Open directory config panel (C on source root)
    OpenDirConfig(PathBuf),

    /// Save dir config edits (Enter on Save button)
    SaveDirConfig,

    /// Close dir config panel (Esc or Discard)
    CloseDirConfig,

    /// Review pending transaction (R key with pending dir config edits)
    ReviewTransaction,
}
