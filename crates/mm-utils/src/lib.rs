//! MM Utilities
//!
//! Shared utilities for Music Magic:
//! - XDG path utilities (config, data, logs)
//! - Audio file extension detection
//! - Metadata normalization ("metadata magic")
//! - UTF-8-safe string helpers

pub mod audio;
pub mod image_hash;
pub mod metadata_magic;
pub mod paths;
pub mod strings;
pub mod tag_names;
pub mod testing;

// Re-export commonly used items at crate root
pub use audio::{is_audio_extension, is_image_extension, AUDIO_EXTENSIONS, IMAGE_EXTENSIONS};
pub use paths::{get_config_dir, get_data_dir, get_db_path, get_logs_dir};
