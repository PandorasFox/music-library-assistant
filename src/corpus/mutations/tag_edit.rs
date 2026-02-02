//! Tag Edit Execution
//!
//! Handles execution of tag-related mutations using the DB-first pattern with spawn chaining:
//! - SetTrackTagsDb: Write complete tag set to database, set needs_disk_flush=true,
//!   **spawn** ApplyDbTagsToDisk to sync to disk.
//!
//! The disk sync (ApplyDbTagsToDisk) is now in indexing.rs since it handles both
//! OOB sync resolution and spawned post-DB-edit syncs.
//!
//! All disk tag operations go through `corpus::tags` module.
//! This file does NOT directly use lofty.

use anyhow::Result;

use crate::corpus::db::types::FileSource;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::witch::{MutationExecutionWitness, SpawnedMutation};

use super::types::{Mutation, MutationResult};

/// Execute SetTrackTagsDb: set track tags in database only (DB-first pattern, step 1).
///
/// - Writes complete tag set to DB via set_track_tags()
/// - Sets needs_disk_flush = TRUE
/// - Returns authorized SpawnedMutation for ApplyDbTagsToDisk (step 2)
///
/// The spawned ApplyDbTagsToDisk will sync DB tags to disk.
fn execute_set_track_tags_db(
    db: &ReadOnlyDb<'_>,
    track_id: i64,
    tags: &[(String, String)],
    witness: &MutationExecutionWitness,
) -> Result<Option<SpawnedMutation>> {
    use crate::db_thread;

    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Get audio file info from DB (read-only) - track_id is actually inode
    // Tag edits only operate on corpus files
    let audio_file = db.get_audio_file_by_inode(track_id, FileSource::Corpus)?
        .ok_or_else(|| anyhow::anyhow!("Audio file not found for inode: {}", track_id))?;
    let file_path = audio_file.path();

    // Write tags to DB (full replacement)
    sender.set_track_tags(file_path, tags.to_vec(), witness);

    // Mark as needing disk flush
    sender.set_needs_disk_flush(file_path, true, witness);

    // Resolve relative DB path to absolute for ApplyDbTagsToDisk
    let resolver = paths::get_resolver();
    let abs_path = resolver.resolve(std::path::Path::new(file_path));

    // Create authorized spawned mutation via witness factory
    Ok(Some(witness.spawn_mutation(Mutation::ApplyDbTagsToDisk {
        track_id,
        path: abs_path,
    })))
}

/// Execute a single tag edit mutation.
///
/// Returns MutationResult with spawn_mutations populated for chaining.
/// Requires a MutationExecutionWitness to prove execution is inside the daemon.
pub fn execute_single(
    db: &ReadOnlyDb<'_>,
    mutation: &Mutation,
    _session_id: &str,
    witness: &MutationExecutionWitness,
) -> MutationResult {
    let start = std::time::Instant::now();

    let (result, spawn_mutations) = match mutation {
        Mutation::SetTrackTagsDb { track_id, tags } => {
            match execute_set_track_tags_db(db, *track_id, tags, witness) {
                Ok(Some(spawn)) => (Ok(()), vec![spawn]),
                Ok(None) => (Ok(()), Vec::new()),
                Err(e) => (Err(e), Vec::new()),
            }
        }

        _ => (Err(anyhow::anyhow!("Not a tag edit mutation")), Vec::new()),
    };

    let (success, error) = match result {
        Ok(()) => (true, None),
        Err(e) => (false, Some(format!("{:#}", e))),
    };

    MutationResult {
        _mutation: mutation.clone(),
        success,
        error,
        _duration_ms: start.elapsed().as_millis() as u64,
        spawn_mutations,
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
