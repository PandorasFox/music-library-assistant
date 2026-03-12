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

/// MM release version string (shown in title bar and reports)
pub const MM_VERSION: &str = "beta 9";

/// Full application title with version
pub const MM_TITLE: &str = "Music Magic (mm beta 9)";

mod config;
mod corpus;
mod db;
mod external;
mod logging;
mod meta;
mod ui;
mod witch;
pub mod zones;

use anyhow::Result;

fn main() -> Result<()> {
    // Step 0: Initialize log channel FIRST (before any logging happens)
    let log_rx = logging::init_log_channel();

    // Step 1: Ensure config directory exists
    let config_dir = config::get_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;

    // Step 2: First-time setup if no config exists (pre-Witch — She needs a config to boot)
    if !config::config_exists() {
        ui::startup::run_first_time_setup()?;
    }

    // Step 3: Load config (parse KDL)
    let config = match config::load_config() {
        Ok(cfg) => {
            logging::log_general("Config loaded successfully");
            config::init_performance_config(cfg.opinions.performance.clone());
            cfg
        }
        Err(e) => {
            eprintln!("ERROR: Failed to load config\n");
            eprintln!("{:#}", e);
            std::process::exit(1);
        }
    };

    // Step 4: Validate config (filesystem tests - same-device check for hardlinks)
    if let Err(e) = config.validate() {
        eprintln!("ERROR: Config validation failed\n");
        eprintln!("{:#}", e);
        std::process::exit(1);
    }

    // Step 5: Clear terminal
    print!("\x1B[2J\x1B[1;1H");

    // Step 6: Witch owns the main thread. TUI is spawned as a client thread.
    let force_check = config.opinions.startup.force_check_all_files_at_startup;
    let shared_config = config.into_shared();

    // Clone config for the Witch before moving shared_config into the closure
    let cfg = config::read_shared_config(&shared_config).clone();
    witch::Witch::run(&cfg, force_check, Some(log_rx), move |handle, cache, notices| {
        if let Err(e) = ui::run_tui(shared_config, handle, cache, notices) {
            eprintln!("TUI error: {:?}", e);
        }
    });

    Ok(())
}
