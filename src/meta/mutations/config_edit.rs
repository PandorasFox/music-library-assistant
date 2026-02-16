//! Config Edit Mutation
//!
//! Writes edited config to disk (comment-preserving KDL modification).
//! Returns the new Config in-band via TaskResult.config_update.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::meta::computations::Computation;
use crate::meta::mutations::types::{MutationResult, SignalClearScope, SignalToClear};
use super::traits::{MutationContext, MutationExecutor};

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
// Config edits are always unique (compare by original_kdl identity).
impl PartialEq for ApplyConfigEditsMutation {
    fn eq(&self, other: &Self) -> bool {
        self.original_kdl == other.original_kdl
    }
}

impl MutationExecutor for ApplyConfigEditsMutation {
    fn label(&self) -> &'static str {
        "Config update"
    }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        match crate::config::write_config_to_disk(&self.original_kdl, &self.old_config, &self.new_config) {
            Ok(()) => {
                crate::logging::log_general("[CONFIG] Config written to disk successfully");
                MutationResult {
                    _mutation: super::Mutation::ApplyConfigEdits(self.clone()),
                    success: true,
                    error: None,
                    _duration_ms: 0, // Overwritten by caller
                    spawn_mutations: Vec::new(),
                    pending_signals: Vec::new(),
                    discovered_inodes: Vec::new(),
                }
            }
            Err(e) => {
                crate::logging::log_error(format!("[CONFIG] Config write failed: {:#}", e));
                MutationResult {
                    _mutation: super::Mutation::ApplyConfigEdits(self.clone()),
                    success: false,
                    error: Some(format!("Config write failed: {:#}", e)),
                    _duration_ms: 0,
                    spawn_mutations: Vec::new(),
                    pending_signals: Vec::new(),
                    discovered_inodes: Vec::new(),
                }
            }
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn additional_computations(&self) -> Vec<Computation> {
        Vec::new()
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        Vec::new()
    }

    fn paths_for_signal_updates(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }
}
