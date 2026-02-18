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
// Version Information
// ============================================================================

/// MM release version string (shown in title bar and reports)
pub const MM_VERSION: &str = "beta 7";

/// Full application title with version
pub const MM_TITLE: &str = "Music Magic (mm beta 7)";

mod config;
mod corpus;
mod db_thread;
mod logging;
mod meta;
mod ui;
mod witch;

use anyhow::Result;

fn main() -> Result<()> {
    // Step 0: Initialize log channel FIRST (before any logging happens)
    let log_rx = logging::init_log_channel();

    // Step 1: Ensure config directory exists
    let config_dir = config::get_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;

    // Step 2: First-time setup if no config exists
    if !config::config_exists() {
        ui::startup::run_first_time_setup()?;
    }

    // Step 3: Load config (parse KDL)
    let config = match config::load_config() {
        Ok(cfg) => {
            // Log successful parse and initialize performance globals
            logging::log_general("Config loaded successfully");
            config::init_performance_config(cfg.opinions.performance.clone());
            cfg
        }
        Err(e) => {
            // Print verbose error to stderr
            eprintln!("ERROR: Failed to load config\n");
            eprintln!("{:#}", e); // Pretty-print anyhow error chain
            std::process::exit(1);
        }
    };

    // Step 4: Validate config (filesystem tests - same-device check for hardlinks)
    if let Err(e) = config.validate() {
        eprintln!("ERROR: Config validation failed\n");
        eprintln!("{:#}", e);
        std::process::exit(1);
    }

    // Step 5: Clear terminal and start TUI
    print!("\x1B[2J\x1B[1;1H"); // ANSI: clear screen + move cursor to top
    ui::run_menu(config, log_rx)?;

    Ok(())
}
