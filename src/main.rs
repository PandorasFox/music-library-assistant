//! Music Library Assistant (MLA)
//!
//! A toolkit of precise, limited tools leveraging a common central database.
//! Main operational areas: scan, report, repair, deploy.

mod config;
mod db;
mod deploy;
mod metadata;
mod progress;
mod reports;
mod scanner;
mod ui;

use anyhow::Result;

fn main() -> Result<()> {
    // Ensure config directory exists
    let config_dir = config::get_config_dir()?;
    std::fs::create_dir_all(&config_dir)?;

    // Launch interactive menu
    ui::run_menu()?;

    Ok(())
}
