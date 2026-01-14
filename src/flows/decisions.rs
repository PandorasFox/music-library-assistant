//! Operator-driven decision types.
//!
//! All corpus-mutating operations are modelled as composable decisions
//! that the operator reviews and commits.

use serde::{Deserialize, Serialize};

/// Represents a pending operator decision about the corpus.
/// Decisions are accumulated, previewed, and then executed in batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingDecision {
    pub decision_type: DecisionType,
    pub source_path: String,
    pub target_path: Option<String>,
    pub metadata: Option<String>, // JSON-encoded context (tag changes, etc.)
}

impl PendingDecision {
    /// Create a new decision of the given type.
    pub fn new(decision_type: DecisionType) -> Self {
        Self {
            decision_type,
            source_path: String::new(),
            target_path: None,
            metadata: None,
        }
    }

    /// Set the source path.
    pub fn source(mut self, path: impl Into<String>) -> Self {
        self.source_path = path.into();
        self
    }

    /// Set the target path.
    pub fn target(mut self, path: impl Into<String>) -> Self {
        self.target_path = Some(path.into());
        self
    }

    /// Set metadata as JSON.
    pub fn with_metadata(mut self, json: serde_json::Value) -> Self {
        self.metadata = Some(json.to_string());
        self
    }

    /// Create a Deploy decision.
    pub fn deploy(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self::new(DecisionType::Deploy)
            .source(source)
            .target(target)
    }

    /// Create an Undeploy decision.
    pub fn undeploy(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self::new(DecisionType::Undeploy)
            .source(source)
            .target(target)
    }

    /// Create a Redeploy decision (move existing deployment to new path).
    pub fn redeploy(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self::new(DecisionType::Redeploy)
            .source(source)
            .target(target)
    }

    /// Create a TagEdit decision.
    pub fn tag_edit(path: impl Into<String>, changes: serde_json::Value) -> Self {
        Self::new(DecisionType::TagEdit)
            .source(path)
            .with_metadata(changes)
    }

    /// Create a Delete decision (move to stash).
    pub fn delete(source: impl Into<String>, stash_target: impl Into<String>) -> Self {
        Self::new(DecisionType::Delete)
            .source(source)
            .target(stash_target)
    }

    /// Create a DropIndex decision (remove from index only).
    pub fn drop_index(path: impl Into<String>) -> Self {
        Self::new(DecisionType::DropIndex).source(path)
    }
}

/// Type of corpus mutation decision.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum DecisionType {
    Move,      // Move file within corpus
    Delete,    // Move to stash (remove from corpus)
    DropIndex, // Remove from index (file already missing)
    TagEdit,   // Modify metadata
    Deploy,    // Create hard link to library
    Undeploy,  // Remove hard link from library
    Redeploy,  // Relocate existing library link to new path (stale fix)
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pending_decision_builders() {
        let deploy = PendingDecision::deploy("/corpus/file.flac", "/library/file.flac");
        assert_eq!(deploy.decision_type, DecisionType::Deploy);
        assert_eq!(deploy.source_path, "/corpus/file.flac");
        assert_eq!(deploy.target_path, Some("/library/file.flac".to_string()));

        let tag_edit = PendingDecision::tag_edit("/corpus/file.flac", serde_json::json!({"artist": "New"}));
        assert_eq!(tag_edit.decision_type, DecisionType::TagEdit);
        assert!(tag_edit.metadata.is_some());

        let delete = PendingDecision::delete("/corpus/file.flac", "/stash/file.flac");
        assert_eq!(delete.decision_type, DecisionType::Delete);

        let drop = PendingDecision::drop_index("/corpus/missing.flac");
        assert_eq!(drop.decision_type, DecisionType::DropIndex);
        assert!(drop.target_path.is_none());
    }
}
