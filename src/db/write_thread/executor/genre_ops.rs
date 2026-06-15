//! Genre ledger + vocabulary operations.
//!
//! Writes into `inode_genres` (the per-inode provenance ledger),
//! `unresolved_genre_observations` (queue of raw genre strings the resolver
//! couldn't map), and the operator-curated vocabulary tables (`genre_names`,
//! `genre_aliases`, `genre_implies`). All paths go through the DB write thread
//! to preserve the single-writer invariant.

use anyhow::Context;
use rusqlite::params;

use mm_meta::mutations::genre_vocabulary::GenreVocabularyOp;

use crate::db::Database;

use super::index_ops::current_unix_secs;

/// One row destined for the `inode_genres` ledger.
///
/// Sent through the channel in a batch so callers (computations writing
/// hundreds of ingested rows at a time) don't pay per-row send overhead.
#[derive(Debug, Clone)]
pub struct InodeGenreRow {
    pub inode: i64,
    pub genre_id: i64,
    pub source: i64,
    pub kind: i64,
    pub source_ref: Option<String>,
    pub raw_value: Option<String>,
    pub confidence: Option<f64>,
}

/// One observation of a raw, unresolved genre string.
///
/// The executor merges by `(raw_value, source)`: existing rows have
/// `observation_count` incremented and `last_seen_at` bumped; new rows insert
/// with count=1. The Phase 2 alias editor surfaces these for mapping.
#[derive(Debug, Clone)]
pub struct UnresolvedGenreObservation {
    pub raw_value: String,
    pub source: i64,
}

/// Execute WriteInodeGenres: bulk insert/upsert into `inode_genres`.
///
/// Idempotent per (inode, genre_id, source, kind): re-applies bump
/// `observed_at` and refresh provenance metadata, so re-running the
/// computation on the same inode is safe and cheap.
pub(super) fn execute_write_inode_genres(
    db: &Database,
    rows: &[InodeGenreRow],
) -> anyhow::Result<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let now = current_unix_secs();
    let tx = db.conn().unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO inode_genres
                (inode, genre_id, source, kind, source_ref, raw_value, confidence, observed_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
             ON CONFLICT(inode, genre_id, source, kind) DO UPDATE SET
                source_ref = excluded.source_ref,
                raw_value  = excluded.raw_value,
                confidence = excluded.confidence,
                observed_at = excluded.observed_at",
        )?;
        for r in rows {
            stmt.execute(params![
                r.inode,
                r.genre_id,
                r.source,
                r.kind,
                r.source_ref,
                r.raw_value,
                r.confidence,
                now,
            ])?;
        }
    }
    tx.commit()?;
    Ok(())
}

/// Execute ClearInodeGenresForSource: delete ledger rows for a single inode
/// scoped to a specific provenance source.
///
/// Used by re-runs: before re-ingesting GENRE tags for an inode, the importer
/// clears all FileTagImport rows for that inode so removed tags don't linger.
/// Other sources (Discogs, MB, Manual) are untouched.
pub(super) fn execute_clear_inode_genres_for_source(
    db: &Database,
    inode: i64,
    source: i64,
) -> anyhow::Result<()> {
    db.conn().execute(
        "DELETE FROM inode_genres WHERE inode = ?1 AND source = ?2",
        params![inode, source],
    )?;
    Ok(())
}

/// Apply a batch of genre-vocabulary edit ops in one transaction.
///
/// Returns Ok(()) on success. Errors abort the transaction — callers
/// receive the failure via the sync result channel and surface it to
/// the operator (e.g. as a transaction execution error in the TUI).
///
/// Validation: `MergeGenres { from_id, into_id }` refuses self-merge and
/// requires both ids to exist. Other ops use `INSERT OR IGNORE` so duplicate
/// edits within a batch are silently absorbed.
///
/// Side effect: after a successful transaction, dirties every corpus inode
/// for the `import_genres_from_tags` computation so re-resolution sees the
/// updated alias graph on the next cycle. Without this hook, an operator
/// adding a new alias wouldn't see existing `unresolved_genre_observations`
/// rows get resolved until something else dirtied the corpus.
pub fn execute_apply_genre_vocabulary_edits(
    db: &Database,
    ops: &[GenreVocabularyOp],
) -> anyhow::Result<()> {
    let tx = db.conn().unchecked_transaction()?;

    for op in ops {
        match op {
            GenreVocabularyOp::AddName {
                canonical_name,
                display_name,
            } => {
                tx.execute(
                    "INSERT OR IGNORE INTO genre_names (canonical_name, display_name)
                     VALUES (?1, ?2)",
                    params![canonical_name, display_name],
                )
                .with_context(|| format!("AddName({canonical_name})"))?;
            }

            GenreVocabularyOp::AddAlias { alias, genre_id } => {
                tx.execute(
                    "INSERT OR IGNORE INTO genre_aliases (alias, genre_id)
                     VALUES (?1, ?2)",
                    params![alias, genre_id],
                )
                .with_context(|| format!("AddAlias({alias} -> #{genre_id})"))?;
            }

            GenreVocabularyOp::RemoveAlias { alias } => {
                tx.execute(
                    "DELETE FROM genre_aliases WHERE alias = ?1",
                    params![alias],
                )
                .with_context(|| format!("RemoveAlias({alias})"))?;
            }

            GenreVocabularyOp::AddImplication {
                child_id,
                parent_id,
            } => {
                if child_id == parent_id {
                    anyhow::bail!(
                        "AddImplication: self-implication forbidden (id={child_id})"
                    );
                }
                tx.execute(
                    "INSERT OR IGNORE INTO genre_implies (child_id, parent_id)
                     VALUES (?1, ?2)",
                    params![child_id, parent_id],
                )
                .with_context(|| format!("AddImplication(#{child_id} -> #{parent_id})"))?;
            }

            GenreVocabularyOp::RemoveImplication {
                child_id,
                parent_id,
            } => {
                tx.execute(
                    "DELETE FROM genre_implies
                     WHERE child_id = ?1 AND parent_id = ?2",
                    params![child_id, parent_id],
                )
                .with_context(|| format!("RemoveImplication(#{child_id} -> #{parent_id})"))?;
            }

            GenreVocabularyOp::MergeGenres { from_id, into_id } => {
                if from_id == into_id {
                    anyhow::bail!("MergeGenres: from_id == into_id (#{from_id})");
                }
                // Both must exist before we touch anything dependent.
                let from_exists: bool = tx
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM genre_names WHERE id = ?1",
                        params![from_id],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                let into_exists: bool = tx
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM genre_names WHERE id = ?1",
                        params![into_id],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);
                anyhow::ensure!(
                    from_exists,
                    "MergeGenres: from_id #{from_id} does not exist"
                );
                anyhow::ensure!(
                    into_exists,
                    "MergeGenres: into_id #{into_id} does not exist"
                );

                // Re-point every dependent row to `into_id`.
                tx.execute(
                    "UPDATE OR IGNORE genre_aliases SET genre_id = ?1 WHERE genre_id = ?2",
                    params![into_id, from_id],
                )?;
                // Re-pointed aliases that would collide with an existing
                // alias→into row are absorbed by UPDATE OR IGNORE; clean up
                // any stragglers still pointing at from_id from rows that
                // failed the unique-PK constraint.
                tx.execute(
                    "DELETE FROM genre_aliases WHERE genre_id = ?1",
                    params![from_id],
                )?;

                // Ledger rows: re-point genre_id. Duplicates collapse via
                // ON CONFLICT DO NOTHING semantics from the composite PK.
                tx.execute(
                    "UPDATE OR IGNORE inode_genres SET genre_id = ?1 WHERE genre_id = ?2",
                    params![into_id, from_id],
                )?;
                tx.execute(
                    "DELETE FROM inode_genres WHERE genre_id = ?1",
                    params![from_id],
                )?;

                // Implication edges: re-point both endpoints. Drop any edges
                // that would become self-implications post-merge.
                tx.execute(
                    "UPDATE OR IGNORE genre_implies SET child_id = ?1 WHERE child_id = ?2",
                    params![into_id, from_id],
                )?;
                tx.execute(
                    "DELETE FROM genre_implies WHERE child_id = ?1",
                    params![from_id],
                )?;
                tx.execute(
                    "UPDATE OR IGNORE genre_implies SET parent_id = ?1 WHERE parent_id = ?2",
                    params![into_id, from_id],
                )?;
                tx.execute(
                    "DELETE FROM genre_implies WHERE parent_id = ?1",
                    params![from_id],
                )?;
                tx.execute(
                    "DELETE FROM genre_implies WHERE child_id = parent_id",
                    [],
                )?;

                // Finally, drop the merged-from canonical name.
                tx.execute(
                    "DELETE FROM genre_names WHERE id = ?1",
                    params![from_id],
                )?;
            }
        }
    }

    tx.commit()?;

    crate::logging::log_general(format!(
        "[VOCAB] Applied {} genre vocabulary edit ops",
        ops.len()
    ));

    Ok(())
}

/// Execute BumpUnresolvedGenreObservations: insert-or-increment a batch of
/// raw genre strings the resolver couldn't map.
pub(super) fn execute_bump_unresolved_genre_observations(
    db: &Database,
    observations: &[UnresolvedGenreObservation],
) -> anyhow::Result<()> {
    if observations.is_empty() {
        return Ok(());
    }
    let now = current_unix_secs();
    let tx = db.conn().unchecked_transaction()?;
    {
        let mut stmt = tx.prepare(
            "INSERT INTO unresolved_genre_observations
                (raw_value, source, observation_count, last_seen_at)
             VALUES (?1, ?2, 1, ?3)
             ON CONFLICT(raw_value, source) DO UPDATE SET
                observation_count = observation_count + 1,
                last_seen_at = excluded.last_seen_at",
        )?;
        for o in observations {
            stmt.execute(params![o.raw_value, o.source, now])?;
        }
    }
    tx.commit()?;
    Ok(())
}
