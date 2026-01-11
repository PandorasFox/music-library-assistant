//! Conversational decision flow types.
//!
//! These types support the MLA dialogue system where the librarian
//! works through a stack of decisions presented by the assistant.

#![allow(dead_code)]

use super::changes::PendingChange;

/// Priority level for decisions - determines presentation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum DecisionPriority {
    /// Immediate action recommended (e.g., clear duplicates with bitrate differential)
    High,
    /// Action beneficial but not urgent (e.g., similar metadata, needs review)
    Medium,
    /// Minor cleanup opportunity (e.g., orphaned files, edge cases)
    Low,
}

impl DecisionPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionPriority::High => "high",
            DecisionPriority::Medium => "medium",
            DecisionPriority::Low => "low",
        }
    }
}

/// Category of decision being presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionCategory {
    /// Fingerprint-based duplicate detection
    FingerprintDuplicate,
    /// Metadata-based duplicate (same artist/album/title)
    MetadataDuplicate,
    /// File with quality issues (low bitrate, missing tags)
    QualityIssue,
    /// Orphaned file in lost-files needing disposition
    OrphanDisposition,
}

impl DecisionCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            DecisionCategory::FingerprintDuplicate => "fingerprint_duplicate",
            DecisionCategory::MetadataDuplicate => "metadata_duplicate",
            DecisionCategory::QualityIssue => "quality_issue",
            DecisionCategory::OrphanDisposition => "orphan_disposition",
        }
    }
}

/// A decision presented to the operator in the conversational flow.
#[derive(Debug, Clone)]
pub struct Decision {
    /// Unique identifier for this decision
    pub id: String,
    /// Priority determines presentation order (high first)
    pub priority: DecisionPriority,
    /// Category of decision
    pub category: DecisionCategory,
    /// Human-readable summary of the situation
    pub summary: String,
    /// Detailed explanation shown on request
    pub details: String,
    /// Affected file paths
    pub affected_paths: Vec<String>,
    /// Recommended action (if any)
    pub recommendation: Option<String>,
    /// Pending changes that would be created if approved
    pub pending_changes: Vec<PendingChange>,
    /// Impact metrics (e.g., "3 files, 45MB")
    pub impact_summary: String,
}

/// Operator's response to a decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecisionOutcome {
    /// Accept the recommended action
    Accept,
    /// Reject/skip this decision
    Reject,
    /// Defer to later (re-queue at lower priority)
    Defer,
    /// Accept and apply pattern to similar decisions
    AcceptPattern,
    /// Reject and ignore similar decisions
    RejectPattern,
}

/// A stack of decisions for the operator to work through.
#[derive(Debug, Clone, Default)]
pub struct DecisionStack {
    /// Decisions ordered by priority (high first)
    pub decisions: Vec<Decision>,
    /// Index of current decision being reviewed
    pub current_index: usize,
    /// Decisions that have been resolved
    pub resolved: Vec<(Decision, DecisionOutcome)>,
    /// Auto-ignore patterns learned during session
    pub ignore_patterns: Vec<String>,
}
