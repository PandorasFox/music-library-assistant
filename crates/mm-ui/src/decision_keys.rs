//! DecisionKey constructors — single source of truth for all key construction.
//!
//! Both mm-tui action handlers and mm-web client use these instead of
//! constructing `DecisionKey` variants directly. Resolution-based keys
//! are already covered by `protocol_binding()` on their button types;
//! this module covers the non-resolution keys that have no button.

use std::path::PathBuf;

use mm_meta::decisions::DecisionKey;

// ============================================================================
// Non-resolution keys (no mm-ui button module)
// ============================================================================

pub fn deploy() -> DecisionKey {
    DecisionKey::Deploy
}

pub fn deploy_sidecars() -> DecisionKey {
    DecisionKey::DeploySidecars
}

pub fn config_edit() -> DecisionKey {
    DecisionKey::ConfigEdit
}

pub fn jettison_edit_history() -> DecisionKey {
    DecisionKey::JettisonEditHistory
}

pub fn edit_reversal(session_label: String) -> DecisionKey {
    DecisionKey::EditReversal { session_label }
}

pub fn dir_config_edit(source_path: PathBuf) -> DecisionKey {
    DecisionKey::DirConfigEdit { source_path }
}

pub fn mb_release_approval(release_id: String) -> DecisionKey {
    DecisionKey::MbReleaseApproval { release_id }
}

pub fn tag_edit(key_item: String) -> DecisionKey {
    DecisionKey::TagEdit { key_item }
}

