//! Configuration Module
//!
//! KDL configuration parsing, path utilities, and logging.

#![allow(dead_code)]

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// Canonical list of audio file extensions supported by MLA.
/// All modules should reference this constant to ensure consistency.
pub const AUDIO_EXTENSIONS: &[&str] = &[
    "flac", "mp3", "ogg", "m4a", "opus", "wav", "aiff", "aif", "aac", "wma", "ape", "wv",
];

/// Check if a file extension is a supported audio format.
pub fn is_audio_extension(ext: &str) -> bool {
    AUDIO_EXTENSIONS.contains(&ext.to_lowercase().as_str())
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub corpus_root: PathBuf,
    pub libraries_root: PathBuf,
    pub legacy_library: Option<PathBuf>,
    pub deploy_mappings: Vec<DeployMapping>,
    pub stash_dir: Option<PathBuf>,
    pub opinions: Opinions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct Opinions {
    pub auto_next_save_all: bool,
    pub fingerprint_matching: FingerprintMatchingOpinions,
    pub quality_resolution: QualityResolutionOpinions,
    pub canonicalization: CanonicalizationOpinions,
    pub re_releases: ReReleaseOpinions,
    pub startup: StartupOpinions,
}


/// Opinions for fingerprint matching thresholds
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FingerprintMatchingOpinions {
    /// Duration difference above this % = different track (default: 10.0)
    pub duration_tolerance_percent: f64,
    /// Same dir + different track# = not duplicate (default: true)
    pub require_matching_track_number: bool,
    /// Different albums can still be duplicates (default: false)
    pub require_matching_album: bool,
}

impl Default for FingerprintMatchingOpinions {
    fn default() -> Self {
        Self {
            duration_tolerance_percent: 10.0,
            require_matching_track_number: true,
            require_matching_album: false,
        }
    }
}

/// Opinions for quality-based auto-resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityResolutionOpinions {
    /// FLAC beats MP3 automatically (default: true)
    pub auto_resolve_format_tier: bool,
    /// Bitrate diff above this % = clear winner (default: 50.0)
    pub bitrate_threshold_percent: f64,
}

impl Default for QualityResolutionOpinions {
    fn default() -> Self {
        Self {
            auto_resolve_format_tier: true,
            bitrate_threshold_percent: 50.0,
        }
    }
}

/// Opinions for artist canonicalization
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CanonicalizationOpinions {
    /// Case-insensitive matching (default: true)
    pub case_insensitive: bool,
    /// Strip parentheticals like "(Live)" (default: false)
    pub strip_parentheticals: bool,
    /// Levenshtein similarity threshold (default: 0.85)
    pub fuzzy_threshold: f64,
}

impl Default for CanonicalizationOpinions {
    fn default() -> Self {
        Self {
            case_insensitive: true,
            strip_parentheticals: false,
            fuzzy_threshold: 0.85,
        }
    }
}

/// How to handle re-releases (same audio on different albums)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[derive(Default)]
pub enum ReReleaseHandling {
    /// Mark as known variant, don't flag as duplicate
    #[default]
    MarkVariant,
    /// Ignore completely
    Ignore,
    /// Flag for manual review
    Flag,
}


/// Opinions for re-release handling
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReReleaseOpinions {
    /// How to handle same fingerprint on different albums
    pub same_fingerprint_different_album: ReReleaseHandling,
}

impl Default for ReReleaseOpinions {
    fn default() -> Self {
        Self {
            same_fingerprint_different_album: ReReleaseHandling::MarkVariant,
        }
    }
}

/// Opinions for startup behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupOpinions {
    /// Run heartbeat check at startup (default: true)
    pub heartbeat_on_startup: bool,
}

impl Default for StartupOpinions {
    fn default() -> Self {
        Self {
            heartbeat_on_startup: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployMapping {
    pub corpus_relative_paths: Vec<PathBuf>, // Multiple paths, relative to corpus-root
    pub library_names: Vec<String>,          // Target library names
}

#[derive(Debug, Clone)]
pub struct ScanSource {
    pub name: String,      // Display name
    pub path: PathBuf,     // File system path
    pub source_id: String, // DB identifier
}

impl Config {
    /// Get all scan sources from the config
    pub fn get_scan_sources(&self) -> Vec<ScanSource> {
        let mut sources = vec![ScanSource {
            name: "corpus".to_string(),
            path: self.corpus_root.clone(),
            source_id: "corpus".to_string(),
        }];

        // Enumerate library subdirectories dynamically
        if let Ok(entries) = fs::read_dir(&self.libraries_root) {
            for entry in entries.flatten() {
                if let Ok(metadata) = entry.metadata() {
                    if metadata.is_dir() {
                        if let Some(name) = entry.file_name().to_str() {
                            // Skip hidden directories
                            if !name.starts_with('.') {
                                sources.push(ScanSource {
                                    name: format!("Library: {}", name),
                                    path: entry.path(),
                                    source_id: name.to_string(),
                                });
                            }
                        }
                    }
                }
            }
        }

        if let Some(legacy) = &self.legacy_library {
            sources.push(ScanSource {
                name: "legacy".to_string(),
                path: legacy.clone(),
                source_id: "legacy".to_string(),
            });
        }

        sources
    }

    /// Validate that all configured paths exist and support required operations
    /// Tests hard link capability for deployment and atomic moves to stash
    pub fn validate_same_filesystem(&self) -> Result<()> {
        use std::fs;
        use std::io::Write;
        use tempfile::NamedTempFile;

        // Step 1: Validate corpus_root exists
        if !self.corpus_root.exists() {
            anyhow::bail!(
                "Validation failed: corpus-root does not exist\n\
                 Path: {:?}\n\
                 \n\
                 Please create this directory or update config.kdl",
                self.corpus_root
            );
        }

        // Step 2: Validate libraries_root exists
        if !self.libraries_root.exists() {
            anyhow::bail!(
                "Validation failed: libraries-root does not exist\n\
                 Path: {:?}\n\
                 \n\
                 Please create this directory or update config.kdl",
                self.libraries_root
            );
        }

        // Step 3: Validate stash_dir exists (if configured)
        if let Some(stash_path) = &self.stash_dir {
            if !stash_path.exists() {
                anyhow::bail!(
                    "Validation failed: stash-dir does not exist\n\
                     Path: {:?}\n\
                     \n\
                     Please create this directory or update config.kdl",
                    stash_path
                );
            }
        }

        // Step 4: Test hard link capability (corpus → libraries)
        let temp_file = NamedTempFile::new_in(&self.corpus_root)
            .context("Failed to create test file in corpus-root")?;
        temp_file.as_file().write_all(b"hardlink_test")?;

        let link_name = format!(
            "mla-hardlink-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let link_path = self.libraries_root.join(&link_name);

        if let Err(e) = fs::hard_link(temp_file.path(), &link_path) {
            anyhow::bail!(
                "Validation failed: Cannot create hard links\n\
                 \n\
                 Corpus root: {:?}\n\
                 Libraries root: {:?}\n\
                 \n\
                 Error: {}\n\
                 \n\
                 EXPLANATION:\n\
                 MLA's deployment requires hard link support between these directories.\n\
                 \n\
                 COMMON CAUSES:\n\
                 - Directories are on different filesystems/volumes\n\
                 - Filesystem doesn't support hard links (FAT32, exFAT, network mounts)\n\
                 \n\
                 SOLUTION:\n\
                 Move both directories to the same filesystem/volume.",
                self.corpus_root,
                self.libraries_root,
                e
            );
        }
        let _ = fs::remove_file(&link_path); // Cleanup

        // Step 5: Test atomic move capability (corpus → stash)
        if let Some(stash_path) = &self.stash_dir {
            let temp_file2 = NamedTempFile::new_in(&self.corpus_root)
                .context("Failed to create test file in corpus-root")?;

            // Persist the temp file so it doesn't get deleted when we convert it
            let source_path = temp_file2.into_temp_path();

            let move_name = format!(
                "mla-move-test-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            );
            let move_path = stash_path.join(&move_name);

            if let Err(e) = fs::rename(&source_path, &move_path) {
                // Cleanup source file if rename failed
                let _ = fs::remove_file(&source_path);

                anyhow::bail!(
                    "Validation failed: Cannot atomically move files\n\
                     \n\
                     Corpus root: {:?}\n\
                     Stash dir: {:?}\n\
                     \n\
                     Error: {}\n\
                     \n\
                     EXPLANATION:\n\
                     MLA needs atomic moves between these directories.\n\
                     Atomic moves only work on the same filesystem.\n\
                     \n\
                     SOLUTION:\n\
                     Move stash-dir to the same filesystem as corpus-root.",
                    self.corpus_root,
                    stash_path,
                    e
                );
            }
            let _ = fs::remove_file(&move_path); // Cleanup
        }

        // Step 6: Log success
        log_message("Filesystem validation passed")?;
        Ok(())
    }

    /// Validate deployment configuration
    pub fn validate(&self) -> Result<()> {
        // Validate filesystem consistency for hard links
        self.validate_same_filesystem()
            .context("Filesystem validation failed")?;

        // Validate deploy mappings
        for mapping in &self.deploy_mappings {
            // Note: We can't validate library names here because libraries are
            // discovered at runtime by scanning libraries_root subdirectories

            // Check that deploy paths are within corpus root
            for corpus_path in &mapping.corpus_relative_paths {
                let full_path = self.corpus_root.join(corpus_path);
                if !full_path.starts_with(&self.corpus_root) {
                    anyhow::bail!("Deploy path escapes corpus root: {:?}", corpus_path);
                }
            }
        }

        Ok(())
    }
}

/// Get the config directory path ($XDG_CONFIG_HOME/mla/ or ~/.config/mla/)
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

    Ok(config_home.join("mla"))
}

/// Get the data directory path ($XDG_DATA_HOME/mla/ or ~/.local/share/mla/)
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

    let data_dir = data_home.join("mla");

    // Ensure the directory exists
    if !data_dir.exists() {
        fs::create_dir_all(&data_dir).context("Failed to create data directory")?;
    }

    Ok(data_dir)
}

/// Get the database path (~/.local/share/mla/mla.db)
pub fn get_db_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("mla.db"))
}

/// Get the log path for stderr-type logging and warnings
/// Now located in XDG data directory: ~/.local/share/mla/mla.log
pub fn get_log_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("mla.log"))
}

/// Get the path for operation summary logs
/// Located at ~/.local/share/mla/operations-overview.log
pub fn get_operations_log_path() -> Result<PathBuf> {
    Ok(get_data_dir()?.join("operations-overview.log"))
}

/// Log a message (error, warning, etc.) to the main log file
pub fn log_message(message: &str) -> Result<()> {
    use std::io::Write;

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

/// Log a scan error to the log file
/// Deprecated alias for log_message, kept for backward compatibility
pub fn log_scan_error(message: &str) -> Result<()> {
    log_message(message)
}

/// Load config from $XDG_CONFIG_HOME/mla/config.kdl (or ~/.config/mla/config.kdl if XDG_CONFIG_HOME is not set)
pub fn load_config() -> Result<Config> {
    let config_dir = get_config_dir()?;
    let config_path = config_dir.join("config.kdl");

    if !config_path.exists() {
        anyhow::bail!(
            "config.kdl not found at {:?}\n\
            Please create a config file at $XDG_CONFIG_HOME/mla/config.kdl (or ~/.config/mla/config.kdl)",
            config_path
        );
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config from {:?}", config_path))?;

    let config = parse_kdl_config(&content)?;
    config.validate()?;

    Ok(config)
}

/// Parse fingerprint-matching opinions from KDL node
fn parse_fingerprint_matching_opinions(node: &kdl::KdlNode, opinions: &mut FingerprintMatchingOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "duration-tolerance-percent" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.duration_tolerance_percent = val;
                        }
                    }
                }
                "require-matching-track-number" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.require_matching_track_number = val;
                        }
                    }
                }
                "require-matching-album" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.require_matching_album = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse quality-resolution opinions from KDL node
fn parse_quality_resolution_opinions(node: &kdl::KdlNode, opinions: &mut QualityResolutionOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "auto-resolve-format-tier" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.auto_resolve_format_tier = val;
                        }
                    }
                }
                "bitrate-threshold-percent" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.bitrate_threshold_percent = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse canonicalization opinions from KDL node
fn parse_canonicalization_opinions(node: &kdl::KdlNode, opinions: &mut CanonicalizationOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "case-insensitive" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.case_insensitive = val;
                        }
                    }
                }
                "strip-parentheticals" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.strip_parentheticals = val;
                        }
                    }
                }
                "fuzzy-threshold" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.fuzzy_threshold = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse re-release opinions from KDL node
fn parse_rerelease_opinions(node: &kdl::KdlNode, opinions: &mut ReReleaseOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "same-fingerprint-different-album" {
                if let Some(entry) = child.entries().first() {
                    if let Some(val) = entry.value().as_string() {
                        opinions.same_fingerprint_different_album = match val {
                            "mark-variant" => ReReleaseHandling::MarkVariant,
                            "ignore" => ReReleaseHandling::Ignore,
                            "flag" => ReReleaseHandling::Flag,
                            _ => ReReleaseHandling::MarkVariant,
                        };
                    }
                }
            }
        }
    }
}

/// Parse startup opinions from KDL node
fn parse_startup_opinions(node: &kdl::KdlNode, opinions: &mut StartupOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "heartbeat-on-startup" {
                if let Some(entry) = child.entries().first() {
                    if let Some(val) = entry.value().as_bool() {
                        opinions.heartbeat_on_startup = val;
                    }
                }
            }
        }
    }
}

fn parse_kdl_config(content: &str) -> Result<Config> {
    let doc: kdl::KdlDocument = content.parse().context("Failed to parse KDL document")?;

    // Check for old format with library blocks
    let has_old_library_format = doc.nodes().iter().any(|n| n.name().value() == "library");

    if has_old_library_format {
        anyhow::bail!(
            "Old config format detected: 'library' blocks are no longer supported\n\
             \n\
             MIGRATION REQUIRED:\n\
             \n\
             OLD FORMAT:\n\
             library \"music\" {{\n\
                 path \"/archive/libraries/music\"\n\
             }}\n\
             \n\
             NEW FORMAT:\n\
             libraries-root \"/archive/libraries\"\n\
             \n\
             Libraries are now discovered by scanning subdirectories under libraries-root.\n\
             Directory names become library names (e.g., /archive/libraries/music → 'music').\n\
             \n\
             DEPLOY FORMAT ALSO CHANGED to support multiple corpus paths:\n\
             \n\
             OLD:\n\
             deploy \"web/releases/bandcamp\" {{\n\
                 library \"music\"\n\
             }}\n\
             \n\
             NEW:\n\
             deploy \"web/releases/bandcamp\" \"web/releases/itunes\" {{\n\
                 library \"music\"\n\
             }}"
        );
    }

    let mut config = Config {
        corpus_root: PathBuf::new(),
        libraries_root: PathBuf::new(),
        legacy_library: None,
        deploy_mappings: Vec::new(),
        stash_dir: None,
        opinions: Opinions::default(),
    };

    for node in doc.nodes() {
        match node.name().value() {
            // Accept both "corpus-root" (new) and "archive-root" (backward compat)
            "corpus-root" | "archive-root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.corpus_root = PathBuf::from(path_str);
                    }
                }
            }
            "libraries-root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.libraries_root = PathBuf::from(path_str);
                    }
                }
            }
            "legacy-library" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.legacy_library = Some(PathBuf::from(path_str));
                    }
                }
            }
            "deploy" => {
                let mut corpus_paths = Vec::new();

                // Collect all path arguments (multiple corpus paths supported)
                for entry in node.entries() {
                    if let Some(path_str) = entry.value().as_string() {
                        corpus_paths.push(PathBuf::from(path_str));
                    }
                }

                // Parse library names from children
                let mut library_names = Vec::new();
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        if child.name().value() == "library" {
                            if let Some(lib_entry) = child.entries().first() {
                                if let Some(lib_name) = lib_entry.value().as_string() {
                                    library_names.push(lib_name.to_string());
                                }
                            }
                        }
                    }
                }

                if !corpus_paths.is_empty() {
                    config.deploy_mappings.push(DeployMapping {
                        corpus_relative_paths: corpus_paths,
                        library_names,
                    });
                }
            }
            "stash-dir" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.stash_dir = Some(PathBuf::from(path_str));
                    }
                }
            }
            "opinions" => {
                if let Some(children) = node.children() {
                    for child in children.nodes() {
                        match child.name().value() {
                            "auto_next_save_all" => {
                                config.opinions.auto_next_save_all = true;
                            }
                            "fingerprint-matching" => {
                                parse_fingerprint_matching_opinions(child, &mut config.opinions.fingerprint_matching);
                            }
                            "quality-resolution" => {
                                parse_quality_resolution_opinions(child, &mut config.opinions.quality_resolution);
                            }
                            "canonicalization" => {
                                parse_canonicalization_opinions(child, &mut config.opinions.canonicalization);
                            }
                            "re-releases" => {
                                parse_rerelease_opinions(child, &mut config.opinions.re_releases);
                            }
                            "startup" => {
                                parse_startup_opinions(child, &mut config.opinions.startup);
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if config.corpus_root.as_os_str().is_empty() {
        anyhow::bail!("corpus-root not specified in config.kdl");
    }

    if config.libraries_root.as_os_str().is_empty() {
        anyhow::bail!("libraries-root not specified in config.kdl");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_new_format() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"
libraries-root "/Volumes/cerberus/library"
stash-dir "/Volumes/cerberus/archive/stash"

deploy "web/releases/bandcamp" "web/releases/itunes" {
    library "main"
}

legacy-library "/Volumes/cerberus/archive/working/legacy"
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(
            config.corpus_root,
            PathBuf::from("/Volumes/cerberus/archive/music")
        );
        assert_eq!(
            config.libraries_root,
            PathBuf::from("/Volumes/cerberus/library")
        );
        assert_eq!(config.deploy_mappings.len(), 1);
        assert_eq!(config.deploy_mappings[0].corpus_relative_paths.len(), 2);
        assert_eq!(config.deploy_mappings[0].library_names[0], "main");
        assert!(config.legacy_library.is_some());
        assert!(config.stash_dir.is_some());
    }

    #[test]
    fn test_old_format_rejected() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"

library "main" {
    path "/Volumes/cerberus/library/main"
}
"#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("Old config format"));
        assert!(err_msg.contains("MIGRATION REQUIRED"));
    }

    #[test]
    fn test_missing_libraries_root() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"
"#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("libraries-root not specified"));
    }

    #[test]
    fn test_opinions_parsing() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"
libraries-root "/Volumes/cerberus/library"

opinions {
    auto_next_save_all

    fingerprint-matching {
        duration-tolerance-percent 15.0
        require-matching-track-number false
        require-matching-album true
    }

    quality-resolution {
        auto-resolve-format-tier false
        bitrate-threshold-percent 75.0
    }

    canonicalization {
        case-insensitive false
        strip-parentheticals true
        fuzzy-threshold 0.90
    }

    re-releases {
        same-fingerprint-different-album "flag"
    }
}
"#;

        let config = parse_kdl_config(kdl).unwrap();

        // Check auto_next_save_all flag
        assert!(config.opinions.auto_next_save_all);

        // Check fingerprint matching
        assert_eq!(config.opinions.fingerprint_matching.duration_tolerance_percent, 15.0);
        assert!(!config.opinions.fingerprint_matching.require_matching_track_number);
        assert!(config.opinions.fingerprint_matching.require_matching_album);

        // Check quality resolution
        assert!(!config.opinions.quality_resolution.auto_resolve_format_tier);
        assert_eq!(config.opinions.quality_resolution.bitrate_threshold_percent, 75.0);

        // Check canonicalization
        assert!(!config.opinions.canonicalization.case_insensitive);
        assert!(config.opinions.canonicalization.strip_parentheticals);
        assert_eq!(config.opinions.canonicalization.fuzzy_threshold, 0.90);

        // Check re-releases
        assert_eq!(config.opinions.re_releases.same_fingerprint_different_album, ReReleaseHandling::Flag);
    }

    #[test]
    fn test_opinions_defaults() {
        let kdl = r#"
corpus-root "/Volumes/cerberus/archive/music"
libraries-root "/Volumes/cerberus/library"
"#;

        let config = parse_kdl_config(kdl).unwrap();

        // Should have default values
        assert!(!config.opinions.auto_next_save_all);
        assert_eq!(config.opinions.fingerprint_matching.duration_tolerance_percent, 10.0);
        assert!(config.opinions.fingerprint_matching.require_matching_track_number);
        assert!(!config.opinions.fingerprint_matching.require_matching_album);
        assert!(config.opinions.quality_resolution.auto_resolve_format_tier);
        assert_eq!(config.opinions.quality_resolution.bitrate_threshold_percent, 50.0);
        assert!(config.opinions.canonicalization.case_insensitive);
        assert!(!config.opinions.canonicalization.strip_parentheticals);
        assert_eq!(config.opinions.canonicalization.fuzzy_threshold, 0.85);
        assert_eq!(config.opinions.re_releases.same_fingerprint_different_album, ReReleaseHandling::MarkVariant);
    }
}
