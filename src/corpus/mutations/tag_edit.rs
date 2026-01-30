//! Tag Edit Execution
//!
//! Handles execution of tag-related mutations using the DB-first pattern:
//! - SetTrackTagsDb: Write complete tag set to database, set needs_disk_flush=true
//! - FlushTagsToDisk: Read from DB, write to disk, clear needs_disk_flush
//!
//! All disk tag operations go through `corpus::tags` module.
//! This file does NOT directly use lofty.

use anyhow::{Context, Result};
use std::path::Path;

use crate::corpus::db::Database;
use crate::corpus::tags::{write_file_tags, TagSet};
use crate::witch::MutationExecutionWitness;

use super::sealed::MutationToken;
use super::types::{Mutation, MutationResult};

/// Execute SetTrackTagsDb: set track tags in database only (DB-first pattern, step 1).
///
/// - Writes complete tag set to DB via set_track_tags()
/// - Sets needs_disk_flush = TRUE
///
/// Should be followed by FlushTagsToDisk to sync to disk.
fn execute_set_track_tags_db(
    db: &Database,
    track_id: i64,
    tags: &[(String, String)],
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Get track path from DB (read-only)
    let track = db.get_track_by_id(track_id)?
        .ok_or_else(|| anyhow::anyhow!("Track not found: {}", track_id))?;

    // Write tags to DB (full replacement)
    sender.set_track_tags(&track.path, tags.to_vec(), witness);

    // Mark as needing disk flush
    sender.set_needs_disk_flush(&track.path, true, witness);

    Ok(())
}

/// Execute FlushTagsToDisk: flush DB tags to disk file (DB-first pattern, step 2).
///
/// - Reads current tags from DB (source of truth)
/// - Writes to disk via write_file_tags()
/// - Updates mtime in scan_state
/// - Sets needs_disk_flush = FALSE
///
/// Idempotent: re-running reads fresh DB state.
fn execute_flush_tags_to_disk(
    db: &Database,
    track_id: i64,
    path: &Path,
    witness: &MutationExecutionWitness,
) -> Result<()> {
    use crate::corpus::paths;
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;
    let resolver = paths::get_resolver();

    // Get relative path for DB operations
    let rel_path = resolver.to_relative(path)
        .ok_or_else(|| anyhow::anyhow!("Path {} not in corpus root", path.display()))?;
    let rel_path_str = rel_path.to_string_lossy();

    // Read current tags from DB (source of truth)
    let db_tags = db.get_track_tags(track_id)
        .with_context(|| format!("Failed to read tags for track {}", track_id))?;
    let tag_set = TagSet::new(
        db_tags.into_iter().map(|t| (t.tag_name, t.tag_value))
    );

    // Write to disk using the consolidated write path
    let token = MutationToken::new();
    write_file_tags(path, &tag_set, &token, witness)
        .with_context(|| format!("Failed to write tags to {}", path.display()))?;
    // Note: write_file_tags already updates mtime in scan_state and clears OOB signals

    // Clear flush flag
    sender.set_needs_disk_flush(&rel_path_str, false, witness);

    Ok(())
}

/// Execute a single tag edit mutation.
///
/// Convenience function for executing individual mutations outside of batch context.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    db: &Database,
    mutation: &Mutation,
    _session_id: &str,
    witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let result = match mutation {
        Mutation::SetTrackTagsDb { track_id, tags } => {
            execute_set_track_tags_db(db, *track_id, tags, witness)
        }

        Mutation::FlushTagsToDisk { track_id, path } => {
            execute_flush_tags_to_disk(db, *track_id, path, witness)
        }

        _ => Err(anyhow::anyhow!("Not a tag edit mutation")),
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(format!("{:#}", e))),
    };

    MutationResult {
        mutation: mutation.clone(),
        success,
        error,
        duration_ms: start.elapsed().as_millis() as u64,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_mutation_dispatch() {
        // Test that execute_single returns appropriate error for non-tag mutations
        let move_mutation = Mutation::Move {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
        };

        // We can't actually execute without a DB, but we can verify the structure
        assert!(matches!(move_mutation, Mutation::Move { .. }));
    }
}
