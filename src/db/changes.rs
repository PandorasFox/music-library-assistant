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
    Move,     // Move file within corpus
    Delete,   // Move to lost-files
    TagEdit,  // Modify metadata
    Deploy,   // Create hard link to library
    Undeploy, // Remove hard link from library
}

impl ChangeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ChangeType::Move => "move",
            ChangeType::Delete => "delete",
            ChangeType::TagEdit => "tag_edit",
            ChangeType::Deploy => "deploy",
            ChangeType::Undeploy => "undeploy",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "move" => Some(ChangeType::Move),
            "delete" => Some(ChangeType::Delete),
            "tag_edit" => Some(ChangeType::TagEdit),
            "deploy" => Some(ChangeType::Deploy),
            "undeploy" => Some(ChangeType::Undeploy),
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
