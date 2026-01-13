//! File-based logging utilities.
//!
//! Simple timestamped logging to a file for errors, warnings, and messages.

use anyhow::Result;
use std::fs;
use std::io::Write;

use crate::paths::get_log_path;

/// Log a message (error, warning, etc.) to the main log file.
///
/// Messages are timestamped and appended to ~/.local/share/mla/mla.log
pub fn log_message(message: &str) -> Result<()> {
    let log_path = get_log_path()?;

    // Ensure parent directory exists
    if let Some(parent) = log_path.parent() {
        fs::create_dir_all(parent)?;
    }

    // Append to log file with timestamp
    let timestamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
    let log_entry = format!("[{}] {}\n", timestamp, message);

    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;

    file.write_all(log_entry.as_bytes())?;

    Ok(())
}

/// Log a scan error to the log file.
///
/// Alias for `log_message`, kept for semantic clarity.
pub fn log_scan_error(message: &str) -> Result<()> {
    log_message(message)
}
