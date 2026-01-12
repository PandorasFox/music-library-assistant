//! Algebraic change tracking types.
//!
//! All corpus-mutating operations are tracked as composable, reversible functions.

#![allow(dead_code)]

use serde::{Deserialize, Serialize};

/// Represents a pending operation on the corpus.
/// All corpus-mutating operations are tracked as composable, reversible functions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingChange {
    pub id: Option<i64>,
    pub session_id: String,
    pub change_type: ChangeType,
    pub source_path: String,
    pub target_path: Option<String>,
    pub metadata_changes: Option<String>, // JSON-encoded tag changes
    pub created_at: Option<String>,
    pub status: ChangeStatus,
}

impl Default for PendingChange {
    fn default() -> Self {
        Self {
            id: None,
            session_id: String::new(),
            change_type: ChangeType::Move,
            source_path: String::new(),
            target_path: None,
            metadata_changes: None,
            created_at: None,
            status: ChangeStatus::Pending,
        }
    }
}

/// Type of corpus mutation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum ChangeType {
    Move,               // Move file within corpus
    Delete,             // Move to stash (remove from corpus)
    DropIndex,          // Remove from index (file already missing)
    TagEdit,            // Modify metadata (user-initiated)
    Deploy,             // Create hard link to library
    Undeploy,           // Remove hard link from library
    Redeploy,           // Relocate existing library link to new path (stale fix)
    OutOfBandTagChange, // Corpus file tags differ from index (detected during scan)
    // TODO: OutOfBandTagChange health check resolution UI
    // - Resolution options: flush index to disk OR accept out-of-band changes
    // - Granularity TBD (potentially per-directory config)
    // - Similar pattern to canon_flow (bucket selection -> confirmation -> commit)
}

impl ChangeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeType::Move => "move",
            ChangeType::Delete => "delete",
            ChangeType::DropIndex => "drop_index",
            ChangeType::TagEdit => "tag_edit",
            ChangeType::Deploy => "deploy",
            ChangeType::Undeploy => "undeploy",
            ChangeType::Redeploy => "redeploy",
            ChangeType::OutOfBandTagChange => "out_of_band_tag_change",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "move" => Some(ChangeType::Move),
            "delete" => Some(ChangeType::Delete),
            "drop_index" => Some(ChangeType::DropIndex),
            "tag_edit" => Some(ChangeType::TagEdit),
            "deploy" => Some(ChangeType::Deploy),
            "undeploy" => Some(ChangeType::Undeploy),
            "redeploy" => Some(ChangeType::Redeploy),
            "out_of_band_tag_change" => Some(ChangeType::OutOfBandTagChange),
            _ => None,
        }
    }
}

/// Status of a pending change.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum ChangeStatus {
    Pending,   // Not yet executed
    Staged,    // Preview created
    Committed, // Executed on filesystem
    Reverted,  // Undone
}

impl ChangeStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeStatus::Pending => "pending",
            ChangeStatus::Staged => "staged",
            ChangeStatus::Committed => "committed",
            ChangeStatus::Reverted => "reverted",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(ChangeStatus::Pending),
            "staged" => Some(ChangeStatus::Staged),
            "committed" => Some(ChangeStatus::Committed),
            "reverted" => Some(ChangeStatus::Reverted),
            _ => None,
        }
    }
}

/// A session groups related changes together for atomic commit/revert.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChangeSession {
    pub session_id: String,
    pub description: String,
    pub created_at: String,
    pub committed_at: Option<String>,
    pub status: String, // "active", "committed", "reverted"
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_change_type_as_str() {
        assert_eq!(ChangeType::Move.as_str(), "move");
        assert_eq!(ChangeType::Delete.as_str(), "delete");
        assert_eq!(ChangeType::TagEdit.as_str(), "tag_edit");
        assert_eq!(ChangeType::Deploy.as_str(), "deploy");
        assert_eq!(ChangeType::Undeploy.as_str(), "undeploy");
        assert_eq!(ChangeType::Redeploy.as_str(), "redeploy");
        assert_eq!(ChangeType::OutOfBandTagChange.as_str(), "out_of_band_tag_change");
    }

    #[test]
    fn test_change_type_from_str() {
        assert_eq!(ChangeType::from_str("move"), Some(ChangeType::Move));
        assert_eq!(ChangeType::from_str("delete"), Some(ChangeType::Delete));
        assert_eq!(ChangeType::from_str("tag_edit"), Some(ChangeType::TagEdit));
        assert_eq!(ChangeType::from_str("deploy"), Some(ChangeType::Deploy));
        assert_eq!(ChangeType::from_str("undeploy"), Some(ChangeType::Undeploy));
        assert_eq!(ChangeType::from_str("redeploy"), Some(ChangeType::Redeploy));
        assert_eq!(ChangeType::from_str("out_of_band_tag_change"), Some(ChangeType::OutOfBandTagChange));
        assert_eq!(ChangeType::from_str("unknown"), None);
        assert_eq!(ChangeType::from_str(""), None);
    }

    #[test]
    fn test_change_type_roundtrip() {
        let types = [
            ChangeType::Move,
            ChangeType::Delete,
            ChangeType::TagEdit,
            ChangeType::Deploy,
            ChangeType::Undeploy,
            ChangeType::Redeploy,
            ChangeType::OutOfBandTagChange,
        ];
        for t in types {
            assert_eq!(ChangeType::from_str(t.as_str()), Some(t));
        }
    }

    #[test]
    fn test_change_status_as_str() {
        assert_eq!(ChangeStatus::Pending.as_str(), "pending");
        assert_eq!(ChangeStatus::Staged.as_str(), "staged");
        assert_eq!(ChangeStatus::Committed.as_str(), "committed");
        assert_eq!(ChangeStatus::Reverted.as_str(), "reverted");
    }

    #[test]
    fn test_change_status_from_str() {
        assert_eq!(ChangeStatus::from_str("pending"), Some(ChangeStatus::Pending));
        assert_eq!(ChangeStatus::from_str("staged"), Some(ChangeStatus::Staged));
        assert_eq!(ChangeStatus::from_str("committed"), Some(ChangeStatus::Committed));
        assert_eq!(ChangeStatus::from_str("reverted"), Some(ChangeStatus::Reverted));
        assert_eq!(ChangeStatus::from_str("unknown"), None);
    }

    #[test]
    fn test_change_status_roundtrip() {
        let statuses = [
            ChangeStatus::Pending,
            ChangeStatus::Staged,
            ChangeStatus::Committed,
            ChangeStatus::Reverted,
        ];
        for s in statuses {
            assert_eq!(ChangeStatus::from_str(s.as_str()), Some(s));
        }
    }

    #[test]
    fn test_pending_change_default() {
        let change = PendingChange::default();
        assert!(change.id.is_none());
        assert!(change.session_id.is_empty());
        assert_eq!(change.change_type, ChangeType::Move);
        assert!(change.source_path.is_empty());
        assert!(change.target_path.is_none());
        assert!(change.metadata_changes.is_none());
        assert!(change.created_at.is_none());
        assert_eq!(change.status, ChangeStatus::Pending);
    }
}
