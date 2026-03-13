//! Config edit mutation struct.

use serde::{Deserialize, Serialize};

use crate::config::Config;

/// Mutation that applies config edits to disk.
///
/// Carries the original KDL text (for comment-preserving modification),
/// the old config (for diffing), and the new config (to write).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyConfigEditsMutation {
    /// The original KDL text from the config file.
    pub original_kdl: String,
    /// The config as it was before editing (for diffing).
    pub old_config: Config,
    /// The new config with edits applied.
    pub new_config: Config,
}

// Manual PartialEq — Config doesn't derive PartialEq, but Mutation enum requires it.
impl PartialEq for ApplyConfigEditsMutation {
    fn eq(&self, other: &Self) -> bool {
        self.original_kdl == other.original_kdl
    }
}
