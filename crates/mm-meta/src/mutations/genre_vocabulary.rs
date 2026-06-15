//! Genre vocabulary edit mutation.
//!
//! Operator-curated edits to the canonical genre vocabulary tables:
//! `genre_names`, `genre_aliases`, `genre_implies`. The mutation carries a
//! batch of atomic ops; the executor applies them in one transaction on
//! the DB write thread.
//!
//! After execution, the importer re-runs against the updated alias table to
//! resolve previously-unresolved observations and re-key existing ledger
//! rows whose raw values now map to a different canonical id.

use serde::{Deserialize, Serialize};

use super::types::DiffEntry;

/// One atomic edit op against the vocabulary tables.
///
/// All variants are operator-issued through the gesture-gated decision
/// protocol; no automatic computation should ever construct these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GenreVocabularyOp {
    /// Add a canonical genre with its display name. Idempotent at the
    /// vocabulary table level (UNIQUE COLLATE NOCASE on canonical_name);
    /// re-issuing for an existing name is a no-op.
    AddName {
        canonical_name: String,
        display_name: String,
    },

    /// Add an alias pointing to an existing canonical id.
    AddAlias { alias: String, genre_id: i64 },

    /// Remove an alias.
    RemoveAlias { alias: String },

    /// Add an implication edge: `child_id` implies `parent_id`. Many-to-many.
    AddImplication { child_id: i64, parent_id: i64 },

    /// Remove an implication edge.
    RemoveImplication { child_id: i64, parent_id: i64 },

    /// Merge `from_id` into `into_id`: re-point all aliases, all ledger rows,
    /// and all implication edges to `into_id`, then delete `from_id`.
    /// Asymmetric (operator chooses which side survives).
    MergeGenres { from_id: i64, into_id: i64 },
}

impl GenreVocabularyOp {
    fn label(&self) -> &'static str {
        match self {
            Self::AddName { .. } => "Add name",
            Self::AddAlias { .. } => "Add alias",
            Self::RemoveAlias { .. } => "Remove alias",
            Self::AddImplication { .. } => "Add implication",
            Self::RemoveImplication { .. } => "Remove implication",
            Self::MergeGenres { .. } => "Merge genres",
        }
    }

    fn diff_entry(&self) -> DiffEntry {
        match self {
            Self::AddName {
                canonical_name,
                display_name,
            } => DiffEntry::new(
                "[vocab] AddName",
                "(none)",
                format!("{} ({})", canonical_name, display_name),
            ),
            Self::AddAlias { alias, genre_id } => DiffEntry::new(
                "[vocab] AddAlias",
                "(none)",
                format!("{} → #{}", alias, genre_id),
            ),
            Self::RemoveAlias { alias } => {
                DiffEntry::new("[vocab] RemoveAlias", alias, "(removed)")
            }
            Self::AddImplication { child_id, parent_id } => DiffEntry::new(
                "[vocab] AddImplication",
                "(none)",
                format!("#{} → #{}", child_id, parent_id),
            ),
            Self::RemoveImplication { child_id, parent_id } => DiffEntry::new(
                "[vocab] RemoveImplication",
                format!("#{} → #{}", child_id, parent_id),
                "(removed)",
            ),
            Self::MergeGenres { from_id, into_id } => DiffEntry::new(
                "[vocab] MergeGenres",
                format!("#{}", from_id),
                format!("(merged into #{})", into_id),
            ),
        }
    }
}

/// Apply a batch of vocabulary edit ops in a single transaction.
///
/// Idempotency: AddName, AddAlias, and AddImplication use `INSERT OR IGNORE`
/// at the executor level so re-issuing one within a transaction is safe.
/// MergeGenres is destructive (deletes `from_id` after re-pointing) and
/// non-idempotent — the executor MUST guard against self-merge and
/// validate both ids exist.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EditGenreVocabularyMutation {
    pub ops: Vec<GenreVocabularyOp>,
}

impl EditGenreVocabularyMutation {
    pub fn diff_entries(&self) -> Vec<DiffEntry> {
        self.ops.iter().map(GenreVocabularyOp::diff_entry).collect()
    }

    /// Total number of edit ops in this batch — used by the transaction
    /// summary UI ("Staged N vocabulary edits").
    pub fn op_count(&self) -> usize {
        self.ops.len()
    }

    /// Op labels for a per-row summary in the diff view.
    pub fn op_labels(&self) -> Vec<&'static str> {
        self.ops.iter().map(GenreVocabularyOp::label).collect()
    }
}
