//! Configuration Module
//!
//! KDL configuration parsing and path utilities.

mod types;
mod performance;
mod debug;
mod dirs;
mod parse;
mod edit;
pub mod path_schema;

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

// Re-export utilities from mm-utils
pub use mm_utils::{
    get_config_dir, get_db_path, is_audio_extension,
    AUDIO_EXTENSIONS,
};

// Re-export from submodules
pub use types::{
    Config, Opinions, StartupView, InboxOrganizeGranularity,
    SourceDir, SharedConfig, read_shared_config, SidecarDeployMode,
    StartupOpinions, QualityResolutionOpinions, CanonicalizationOpinions,
    HealthDetectionOpinions, PerformanceOpinions, TagSplittingOpinions,
    DuplicateAnalysisOpinions, ReleasePackingOpinions, PackingWeights, InboxOrganizeOpinions, ExternalMatchingConfig,
    DiscExtractionOpinions, AlbumArtOpinions, DebugOpinions,
};
pub use performance::{init_performance_config, get_worker_thread_count, get_db_cache_kb, is_timing_enabled};
pub use debug::{init_debug_config, is_memory_logging_enabled};
pub use dirs::{parse_dirs_kdl, write_dirs_to_disk};
pub use edit::write_config_to_disk;
pub use path_schema::parse_path_schema;

use parse::parse_kdl_config;

/// Check whether a config file exists on disk.
///
/// Returns true if config.kdl is present in the config directory.
/// Used by first-time setup to determine if the wizard should run.
pub fn config_exists() -> bool {
    get_config_dir()
        .map(|dir| dir.join("config.kdl").exists())
        .unwrap_or(false)
}

/// Write a minimal initial config file with just the archive root path.
///
/// Used during first-time setup before the full config system is available.
/// The resulting config.kdl contains only the `root` directive; all other
/// settings use defaults.
pub fn write_initial_config(root: &Path) -> Result<()> {
    let config_dir = get_config_dir()?;
    fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("config.kdl");

    let content = format!(
        "// Music Magic configuration\n\
         // See docs/ for full configuration reference.\n\
         \n\
         root \"{}\"\n",
        root.display()
    );

    fs::write(&config_path, content)
        .with_context(|| format!("Failed to write config to {:?}", config_path))?;

    Ok(())
}

/// Load config from disk. Does NOT validate.
///
/// Filesystem validation (`config.validate()`) should be called once at startup.
/// It doesn't need to be repeated - the filesystem layout won't change at runtime.
pub fn load_config() -> Result<Config> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join("config.kdl");

    if !config_path.exists() {
        anyhow::bail!(
            "config.kdl not found at {:?}\n\
            Please create a config file at $XDG_CONFIG_HOME/mm/config.kdl (or ~/.config/mm/config.kdl)",
            config_path
        );
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config from {:?}", config_path))?;

    let mut config = parse_kdl_config(&content)?;

    // Load dirs.kdl (optional — empty dirs is valid)
    let dirs_path = config_dir.join("dirs.kdl");
    if dirs_path.exists() {
        let dirs_content = fs::read_to_string(&dirs_path)
            .with_context(|| format!("Failed to read dirs from {:?}", dirs_path))?;
        config.source_dirs = parse_dirs_kdl(&dirs_content)?;
    }

    Ok(config)
}
