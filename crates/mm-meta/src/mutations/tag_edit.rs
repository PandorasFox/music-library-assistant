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
