//! Music Library Assistant (MLA)
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

/// MLA release version string (shown in title bar and reports)
pub const MLA_VERSION: &str = "alpha 1";

/// Full application title with version
pub const MLA_TITLE: &str = "Music Library Assistant (mla alpha 1)";

mod config;
mod corpus;
mod db;
mod deduplication;
mod metadata;
mod ops;
mod progress;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    // Step 1: Ensure config directory exists
    let config_dir = config::get_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;

    // Step 2: Load config (parse KDL)
    let config = match config::load_config() {
        Ok(cfg) => {
            // Step 3: Log successful parse
            let _ = config::log_message("Config loaded successfully");
            cfg
        }
        Err(e) => {
            // Print verbose error to stderr
            eprintln!("ERROR: Failed to load config\n");
            eprintln!("{:#}", e); // Pretty-print anyhow error chain
            std::process::exit(1);
        }
    };

    // Step 4: Validate config (filesystem tests)
    if let Err(e) = config.validate() {
        eprintln!("ERROR: Config validation failed\n");
        eprintln!("{:#}", e);
        std::process::exit(1);
    }

    // Step 5: Clear terminal and start TUI
    print!("\x1B[2J\x1B[1;1H"); // ANSI: clear screen + move cursor to top
    ui::run_menu(config)?;

    Ok(())
}
