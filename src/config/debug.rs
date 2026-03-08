//! Global debug config (OnceLock + accessor functions).

use super::types::DebugOpinions;
use std::sync::OnceLock;

static DEBUG_CONFIG: OnceLock<DebugOpinions> = OnceLock::new();

/// Initialize the global debug config. Called once at startup.
pub fn init_debug_config(opinions: DebugOpinions) {
    let _ = DEBUG_CONFIG.set(opinions);
}

/// Check if memory diagnostics logging is enabled.
pub fn is_memory_logging_enabled() -> bool {
    DEBUG_CONFIG
        .get()
        .map(|d| d.memory_logging)
        .unwrap_or(false)
}
