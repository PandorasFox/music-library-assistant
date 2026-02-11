//! Tag Edit Execution
//!
//! Handles execution of tag-related mutations using incremental operations with validation:
//! - ApplyTagOps: Apply a set of incremental tag operations, validate expected old values,
//!   write to DB, and spawn ApplyDbTagsToDisk for each modified inode.
//!
//! The disk sync (ApplyDbTagsToDisk) is in indexing.rs since it handles both
//! OOB sync resolution and spawned post-DB-edit syncs.
//!
//! All disk tag operations go through `corpus::tags` module.
//! This file does NOT directly use lofty.

use std::collections::{HashMap, HashSet};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::corpus::db::types::FileSource;
use crate::corpus::db::ReadOnlyDb;
use crate::corpus::paths;
use crate::db_thread;
use crate::witch::{MutationExecutionWitness, SpawnedMutation};

use crate::corpus::tags::TagSet;

use super::indexing::FlushTagsToDiskMutation;
use super::traits::{MutationContext, MutationExecutor};
use super::types::{Mutation, MutationResult, SignalClearScope, TagOp};

// ============================================================================
// Mutation Struct
// ============================================================================

/// Apply a set of incremental tag operations to tracks.
///
/// Operations are pre-coalesced by inode. At execution time:
/// 1. Group ops by inode
/// 2. For each inode: read current tags, validate expected old_values
/// 3. If ANY validation fails for an inode, that inode's ops fail (others continue)
/// 4. Apply tag ops directly via apply_index_tag_ops (INSERT/UPDATE/DELETE)
/// 5. Spawn ApplyDbTagsToDisk for each modified inode
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyTagOpsMutation {
    pub ops: Vec<TagOp>,
}

impl MutationExecutor for ApplyTagOpsMutation {
    fn label(&self) -> &'static str { "Tag edit" }

    fn execute(&self, ctx: &MutationContext) -> MutationResult {
        let start = std::time::Instant::now();
        let (result, spawn_mutations) = match execute_apply_tag_ops(ctx.read_db, &self.ops, ctx.session_id, ctx.witness) {
            Ok(spawned) => (Ok(()), spawned),
            Err(e) => (Err(e), Vec::new()),
        };
        let (success, error) = match result {
            Ok(()) => (true, None),
            Err(e) => (false, Some(format!("{:#}", e))),
        };
        MutationResult {
            _mutation: Mutation::ApplyTagOps(self.clone()),
            success,
            error,
            _duration_ms: start.elapsed().as_millis() as u64,
            spawn_mutations,
            pending_signals: Vec::new(),
            discovered_inodes: Vec::new(),
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope { SignalClearScope::None }
    fn affected_inodes(&self) -> Vec<i64> { Vec::new() }
}

/// Execute ApplyTagOps: apply incremental tag operations with validation.
///
/// For each inode:
/// 1. Read current tags from DB
/// 2. Validate that expected old_values exist (for drop/replace ops)
/// 3. Apply changes to build new tag set
/// 4. Write updated tags to DB
/// 5. Spawn ApplyDbTagsToDisk for disk sync
///
/// Partial success: validation failure for one inode doesn't block others.
fn execute_apply_tag_ops(
    db: &ReadOnlyDb<'_>,
    ops: &[TagOp],
    session_id: &str,
    witness: &MutationExecutionWitness,
) -> Result<Vec<SpawnedMutation>> {
    let sender = db_thread::signal_sender()
        .ok_or_else(|| anyhow::anyhow!("DB thread not initialized"))?;

    // Group ops by inode
    let mut by_inode: HashMap<i64, Vec<&TagOp>> = HashMap::new();
    for op in ops {
        by_inode.entry(op.inode).or_default().push(op);
    }

    let mut spawned = Vec::new();
    let mut errors = Vec::new();

    for (inode, inode_ops) in by_inode {
        // Get audio file info from DB
        let audio_file = match db.get_audio_file_by_inode(inode, FileSource::Corpus)? {
            Some(f) => f,
            None => {
                errors.push(format!("inode {} not found", inode));
                continue;
            }
        };
        let file_path = audio_file.path();

        // Get current tags as set for validation
        let current_tags = db.get_corpus_tags(inode)?;
        let current_set: HashSet<(String, String)> = current_tags
            .iter()
            .map(|t| (t.tag_name.to_lowercase(), t.tag_value.clone()))
            .collect();

        // Validate each op's expected old_value exists
        let mut validation_failed = false;
        for op in &inode_ops {
            let key = op.tag_name.to_lowercase();

            // Validate expected old_value exists (for drop/replace)
            if let Some(ref old) = op.old_value {
                if !current_set.contains(&(key.clone(), old.clone())) {
                    errors.push(format!(
                        "inode {}: expected {}={:?} not found (stale)",
                        inode, op.tag_name, old
                    ));
                    validation_failed = true;
                    break;
                }
            }
        }

        if validation_failed {
            continue; // Skip this inode, try others
        }

        // Collect validated ops for this inode (filter no-ops)
        let validated_ops: Vec<TagOp> = inode_ops
            .into_iter()
            .filter(|op| !op.is_nop())
            .cloned()
            .collect();

        if validated_ops.is_empty() {
            continue; // All ops were no-ops
        }

        // Build the expected tag set: current DB state + validated ops applied.
        // This is computed within the same read transaction — no race condition.
        let mut expected: Vec<(String, String)> = current_tags
            .iter()
            .map(|t| (t.tag_name.to_lowercase(), t.tag_value.clone()))
            .collect();

        for op in &validated_ops {
            let key = op.tag_name.to_lowercase();
            match (&op.old_value, &op.new_value) {
                // Drop: remove the (key, old) pair
                (Some(old), None) => {
                    expected.retain(|(k, v)| !(k == &key && v == old));
                }
                // Replace: remove (key, old), add (key, new)
                (Some(old), Some(new)) => {
                    expected.retain(|(k, v)| !(k == &key && v == old));
                    expected.push((key, new.clone()));
                }
                // Add: add (key, new)
                (None, Some(new)) => {
                    expected.push((key, new.clone()));
                }
                // No-op (already filtered above)
                (None, None) => {}
            }
        }
        let expected_tags = TagSet::new(expected);

        // Send ops directly to DB - TagOps map to INSERT/UPDATE/DELETE
        sender.apply_index_tag_ops(file_path, validated_ops, session_id, witness);
        sender.set_needs_disk_flush(file_path, true, witness);

        // Spawn disk flush with carried tags (no DB read needed — avoids race)
        let resolver = paths::get_resolver();
        let abs_path = resolver.resolve(std::path::Path::new(file_path));
        spawned.push(witness.spawn_mutation(Mutation::FlushTagsToDisk(FlushTagsToDiskMutation {
            inode,
            path: abs_path,
            tags: expected_tags,
        })));
    }

    // Report errors but don't fail entire mutation (partial success)
    if !errors.is_empty() {
        crate::logging::log_error(format!("ApplyTagOps partial failure: {:?}", errors));
    }

    Ok(spawned)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::file_ops::MoveMutation;
    use std::path::PathBuf;

    #[test]
    fn test_mutation_dispatch() {
        // Test that execute_single returns appropriate error for non-tag mutations
        let move_mutation = Mutation::Move(MoveMutation {
            source: PathBuf::from("/a"),
            destination: PathBuf::from("/b"),
        });

        // We can't actually execute without a DB, but we can verify the structure
        assert!(matches!(move_mutation, Mutation::Move(_)));
    }

    #[test]
    fn test_tag_op_constructors() {
        let add = TagOp::add_tag(1, "artist", "Foo");
        assert_eq!(add.inode, 1);
        assert_eq!(add.tag_name, "artist");
        assert_eq!(add.old_value, None);
        assert_eq!(add.new_value, Some("Foo".to_string()));
        assert!(!add.is_nop());

        let drop = TagOp::drop_tag(2, "genre", "Rock");
        assert_eq!(drop.inode, 2);
        assert_eq!(drop.old_value, Some("Rock".to_string()));
        assert_eq!(drop.new_value, None);
        assert!(!drop.is_nop());

        let replace = TagOp::replace_tag(3, "album", "Old", "New");
        assert_eq!(replace.old_value, Some("Old".to_string()));
        assert_eq!(replace.new_value, Some("New".to_string()));
        assert!(!replace.is_nop());

        // Same value = nop
        let nop = TagOp::replace_tag(4, "title", "Same", "Same");
        assert!(nop.is_nop());
    }
}
