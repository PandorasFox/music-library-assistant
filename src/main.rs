//! Music Magic (MM)
//!
//! A toolkit of precise, limited tools leveraging a common central database.
//! Main operational areas: scan, report, repair, deploy.
//!
//! Organized around librarian workflow cycles:
//! - Insight & Health: Understanding corpus state
//! - Intake: Bringing external material into corpus
//! - Organization: Corpus-mutating operations
//! - Deployment: Publishing to browsable libraries
//! - Operations: Low-level maintenance

// ============================================================================
// Allocator
// ============================================================================

// Use jemalloc instead of glibc malloc. Jemalloc returns freed arenas back to
// the OS more aggressively, which matters a lot for MM's burst-then-idle
// computation pattern (startup computation burst allocates ~3GB, then frees it
// all — glibc malloc would hold those arenas indefinitely).
//
// Configuration: background_thread purges dirty pages on a 1s decay timer.
// Without background_thread, decay only happens on the next allocation.
#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// jemalloc build-time configuration string (overrides the weak _rjem_malloc_conf symbol).
/// tikv-jemallocator uses the _rjem_ prefix internally.
/// background_thread:true — enables the purge thread
/// dirty_decay_ms:1000    — dirty (freed) pages returned to OS after ~1s idle
/// muzzy_decay_ms:5000    — muzzy (decommitted) pages cleaned up after ~5s
#[allow(non_upper_case_globals)]
#[unsafe(export_name = "_rjem_malloc_conf")]
pub static _rjem_malloc_conf: &[u8] =
    b"background_thread:true,dirty_decay_ms:1000,muzzy_decay_ms:5000\0";

// ============================================================================
// Version Information
// ============================================================================

pub use mm_meta::{MM_TITLE, MM_VERSION};

mod auth;
mod config;
mod corpus;
mod db;
mod external;
mod logging;
mod meta;
mod witch;
pub mod zones;

use anyhow::Result;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize log channel FIRST (before any logging happens)
    let log_rx = logging::init_log_channel();

    // Ensure config directory exists
    let config_dir = config::get_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;

    // Witch owns the main thread. Clients connect over Unix socket.
    witch::Witch::run(Some(log_rx)).await;

    Ok(())
}
