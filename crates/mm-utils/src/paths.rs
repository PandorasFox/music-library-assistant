//! XDG Base Directory path utilities.
//!
//! Provides standard locations for MM config, data, and logs
//! following the XDG Base Directory specification.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Get the config directory path ($XDG_CONFIG_HOME/mm/ or ~/.config/mm/)
pub fn get_config_dir() -> Result<PathBuf> {
    let config_home = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .and_then(|s| {
            if s.is_empty() {
                None
            } else {
                Some(PathBuf::from(s))
            }
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
        .context("Failed to determine config directory")?;

    Ok(config_home.join("mm"))
}

/// Get the data directory path ($XDG_DATA_HOME/mm/ or ~/.local/share/mm/)
pub fn get_data_dir() -> Result<PathBuf> {
    let data_home = std::env::var("XDG_DATA_HOME")
        .ok()
        .and_then(|s| {
            if s.is_empty() {
                None
            } else {
                Some(PathBuf::from(s))
            }
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(".local").join("share")))
        .context("Failed to determine data directory")?;

    let data_dir = data_home.join("mm");

    // Ensure the directory exists
    if !data_dir.exists() {
        fs::create_dir_all(&data_dir).context("Failed to create data directory")?;
    }

    Ok(data_dir)
}

/// Get the database path (~/.local/share/mm/mm.db)
pub fn get_db_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("mm.db"))
}

/// Get the logs directory path (~/.local/share/mm/logs/)
///
/// Creates the directory if it doesn't exist.
pub fn get_logs_dir() -> Result<PathBuf> {
    let logs_dir = get_data_dir()?.join("logs");
    if !logs_dir.exists() {
        fs::create_dir_all(&logs_dir).context("Failed to create logs directory")?;
    }
    Ok(logs_dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_paths_are_under_mm() {
        // These tests just verify the path structure, not actual filesystem
        let config = get_config_dir().unwrap();
        assert!(config.ends_with("mm"));

        let data = get_data_dir().unwrap();
        assert!(data.ends_with("mm"));

        let db = get_db_path().unwrap();
        assert!(db.ends_with("mm.db"));

        let logs = get_logs_dir().unwrap();
        assert!(logs.ends_with("logs"));
    }
}
