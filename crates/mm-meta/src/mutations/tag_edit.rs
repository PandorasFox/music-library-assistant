//! Tag edit mutation struct.

use serde::{Deserialize, Serialize};

use crate::db_types::Zone;

use super::types::TagOp;

/// Apply a set of incremental tag operations to tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ApplyTagOpsMutation {
    pub ops: Vec<TagOp>,
    /// Which zone the target files belong to (determines tag table).
    pub zone: Zone,
}

impl ApplyTagOpsMutation {
    pub fn diff_entries(&self) -> Vec<super::types::DiffEntry> {
        self.ops
            .iter()
            .map(|op| {
                super::types::DiffEntry::new(
                    format!("[{}] {}", op.inode, op.tag_name),
                    op.old_value.as_deref().unwrap_or("(none)"),
                    op.new_value.as_deref().unwrap_or("(none)"),
                )
            })
            .collect()
    }
}
