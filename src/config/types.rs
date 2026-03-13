//! Configuration types: structs, enums, Default impls, and Config methods.
//!
//! Type definitions live in mm-meta; re-exported here.
//! Filesystem validation (which touches crate-local state) stays here.

// Re-export all config types from mm-meta
pub use mm_meta::config::{
    read_shared_config, AlbumArtOpinions, CanonicalizationOpinions, Config, CreditRoutingConfig,
    DiscExtractionOpinions, DuplicateAnalysisOpinions, ExternalMatchingConfig,
    HealthDetectionOpinions, InboxOrganizeGranularity, InboxOrganizeOpinions, Opinions,
    PackingWeights, PerformanceOpinions, QualityResolutionOpinions, RelationRouting,
    ReleasePackingOpinions, SharedConfig, SidecarDeployMode, SourceDir, StartupOpinions,
    StartupView, TagSplittingOpinions,
};

use anyhow::{Context, Result};

/// Validate that the archive root exists and all subdirectories are on the same filesystem.
///
/// This check ensures:
/// - Hard links will work (corpus -> libraries)
/// - Atomic moves will work (corpus -> stash)
///
/// Uses st_dev (device ID) comparison rather than creating test files.
/// Runtime checks during corpus walking will detect nested mount points.
pub fn validate_same_filesystem(config: &Config) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    use std::path::PathBuf;

    let corpus_dir = config.corpus_dir();
    let libraries_dir = config.libraries_dir();
    let stash_dir = config.stash_dir();

    // Validate root exists
    if !config.root.exists() {
        anyhow::bail!(
            "Validation failed: archive root does not exist\n\
             Path: {:?}\n\
             \n\
             Please create this directory or update config.kdl",
            config.root
        );
    }

    // Validate subdirectories exist and collect device IDs
    let mut device_ids: Vec<(&str, &PathBuf, u64)> = Vec::new();

    // Check root first
    let root_dev = std::fs::metadata(&config.root)
        .with_context(|| format!("Failed to stat root directory: {:?}", config.root))?
        .dev();
    device_ids.push(("root", &config.root, root_dev));

    for (name, dir) in [
        ("corpus", &corpus_dir),
        ("libraries", &libraries_dir),
        ("stash", &stash_dir),
    ] {
        if !dir.exists() {
            anyhow::bail!(
                "Validation failed: {} directory does not exist\n\
                 Path: {:?}\n\
                 \n\
                 Please create this directory under your archive root.",
                name,
                dir
            );
        }

        let dev = std::fs::metadata(dir)
            .with_context(|| format!("Failed to stat {} directory", name))?
            .dev();
        device_ids.push((name, dir, dev));
    }

    // Check all directories are on the same filesystem
    let (first_name, first_dir, first_dev) = device_ids[0];
    for (name, dir, dev) in &device_ids[1..] {
        if *dev != first_dev {
            anyhow::bail!(
                "Validation failed: directories are on different filesystems\n\
                 \n\
                 {} ({:?}): device {}\n\
                 {} ({:?}): device {}\n\
                 \n\
                 Hard links and atomic moves require the same filesystem.\n\
                 All subdirectories must be on the same volume as the archive root.\n\
                 \n\
                 Note: Nested mount points within these directories will be detected\n\
                 at runtime and trigger read-only safety mode.",
                first_name, first_dir, first_dev, name, dir, dev
            );
        }
    }

    // Store the expected device ID for runtime checks
    crate::corpus::paths::set_expected_device_id(root_dev);

    crate::logging::log_general(format!(
        "Filesystem validation passed (device {})",
        root_dev
    ));
    Ok(())
}

/// Validate configuration: filesystem layout and source path containment.
pub fn validate_config(config: &Config) -> Result<()> {
    validate_same_filesystem(config).context("Filesystem validation failed")?;

    // Validate source paths don't escape corpus
    let corpus_dir = config.corpus_dir();
    for source in &config.source_dirs {
        let full_path = corpus_dir.join(&source.path);
        if !full_path.starts_with(&corpus_dir) {
            anyhow::bail!("Source path escapes corpus directory: {:?}", source.path);
        }
    }

    Ok(())
}
