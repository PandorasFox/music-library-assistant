//! MLA Utilities
//!
//! Shared utilities for the Music Library Assistant:
//! - XDG path utilities (config, data, logs)
//! - File-based logging
//! - Audio file extension detection
//! - Metadata normalization ("metadata magic")

pub mod audio;
pub mod logging;
pub mod metadata_magic;
pub mod paths;

// Re-export commonly used items at crate root
pub use audio::{is_audio_extension, AUDIO_EXTENSIONS};
pub use logging::{log_message, log_scan_error};
pub use paths::{get_config_dir, get_data_dir, get_db_path, get_log_path, get_operations_log_path};
