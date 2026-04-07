//! Configuration Module
//!
//! KDL configuration parsing and path utilities.

mod dirs;
mod edit;
mod env_override;
mod parse;
pub mod path_schema;
mod performance;
mod types;

use anyhow::{Context, Result};
use std::fs;
use std::path::Path;

// Re-export utilities from mm-utils
pub use mm_utils::{get_config_dir, get_db_path, is_audio_extension, AUDIO_EXTENSIONS};

// Re-export from submodules
pub use dirs::{parse_dirs_kdl, write_dirs_to_disk};
pub use edit::write_config_to_disk;
pub use performance::{get_db_cache_kb, get_worker_thread_count, init_performance_config, set_db_cache_kb};
pub use types::{
    Config,
    ExternalMatchingConfig, PackingWeights, PerformanceOpinions, ReleasePackingOpinions, SharedConfig, SidecarDeployMode,
    SourceDir, TagSplittingOpinions,
};

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
/// The resulting config.kdl contains only the `storage-root` directive; all other
/// settings use defaults.
pub fn write_initial_config(root: &Path) -> Result<()> {
    let config_dir = get_config_dir()?;
    fs::create_dir_all(&config_dir)?;
    let config_path = config_dir.join("config.kdl");

    let content = format!(
        "// Music Magic configuration\n\
         // See docs/ for full configuration reference.\n\
         \n\
         storage-root \"{}\"\n",
        root.display()
    );

    fs::write(&config_path, content)
        .with_context(|| format!("Failed to write config to {:?}", config_path))?;

    Ok(())
}

/// Load config from disk, falling back to env-var-only defaults.
///
/// If config.kdl exists, parses it and layers env overrides on top.
/// If config.kdl is absent, constructs a default Config and applies env
/// overrides — this allows fully env-driven configuration (e.g. Docker
/// with a read-only config mount). Requires at least `MM_STORAGE_ROOT`
/// (or `MM_ROOT`) to be set when no config file is present.
///
/// Filesystem validation (`config.validate()`) should be called once at startup.
/// It doesn't need to be repeated - the filesystem layout won't change at runtime.
pub fn load_config() -> Result<Config> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join("config.kdl");

    let mut config = if config_path.exists() {
        let content = fs::read_to_string(&config_path)
            .with_context(|| format!("Failed to read config from {:?}", config_path))?;

        let mut cfg = parse_kdl_config(&content)?;

        // Load dirs.kdl (optional — empty dirs is valid)
        let dirs_path = config_dir.join("dirs.kdl");
        if dirs_path.exists() {
            let dirs_content = fs::read_to_string(&dirs_path)
                .with_context(|| format!("Failed to read dirs from {:?}", dirs_path))?;
            cfg.source_dirs = parse_dirs_kdl(&dirs_content)?;
        }

        cfg
    } else {
        // No config file — start from defaults. Env overrides (below) supply
        // storage_root and any other non-default values.
        crate::logging::log_general(
            "No config.kdl found — using defaults with environment overrides"
        );
        Config {
            storage_root: std::path::PathBuf::new(),
            libraries_root: None,
            stash_root: None,
            source_dirs: Vec::new(),
            opinions: types::Opinions::default(),
        }
    };

    // Layer environment variable overrides on top
    env_override::apply_env_overrides(&mut config);

    // Without a config file, storage_root must come from env
    if config.storage_root.as_os_str().is_empty() {
        anyhow::bail!(
            "No config.kdl at {:?} and MM_STORAGE_ROOT not set.\n\
            Either create a config file or set MM_STORAGE_ROOT (or MM_ROOT) in the environment.",
            config_path
        );
    }

    // Clamp root dir None values to system defaults — None means "inherit
    // from parent" but root has no parent.
    config.clamp_root_defaults();

    Ok(config)
}
