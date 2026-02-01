//! Configuration Module
//!
//! KDL configuration parsing and path utilities.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::sync::OnceLock;

// Re-export utilities from mla-utils for backward compatibility
pub use mla_utils::{
    get_config_dir, get_db_path, is_audio_extension,
    AUDIO_EXTENSIONS,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub root: PathBuf,
    pub legacy_enabled: bool,
    pub deploy_mappings: Vec<DeployMapping>,
    pub opinions: Opinions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[derive(Default)]
pub struct Opinions {
    pub auto_next_save_all: bool,
    pub fingerprint_matching: FingerprintMatchingOpinions,
    pub quality_resolution: QualityResolutionOpinions,
    pub canonicalization: CanonicalizationOpinions,
    pub startup: StartupOpinions,
    pub health_detection: HealthDetectionOpinions,
    pub performance: PerformanceOpinions,
    pub tag_splitting: TagSplittingOpinions,
    pub duplicate_analysis: DuplicateAnalysisOpinions,
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

/// Opinions for startup behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupOpinions {
    /// Run last-stage content analysis once at startup (default: true).
    /// Ensures all detection signals (ShitFormat, duplicates, etc.) are fresh.
    /// This is a one-shot latch: triggers once at startup, then auto-clears.
    /// Set to false to skip content analysis on startup (faster, but signals may be stale).
    pub freshen_last_stage_at_startup: bool,
    /// Force verification of all indexed files at startup, bypassing mtime optimization (default: false).
    /// Catches out-of-band tag changes (external tools modified tags) and corrupt files.
    /// Slower startup but ensures database matches reality.
    pub force_check_all_files_at_startup: bool,
}

impl Default for StartupOpinions {
    fn default() -> Self {
        Self {
            freshen_last_stage_at_startup: true,
            force_check_all_files_at_startup: false,
        }
    }
}

/// Opinions for health detection behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthDetectionOpinions {
    /// Tags that must be present on every track (default: title, album, artist, album_artist)
    pub required_tags: Vec<String>,
}

impl Default for HealthDetectionOpinions {
    fn default() -> Self {
        Self {
            required_tags: vec![
                "title".to_string(),
                "album".to_string(),
                "artist".to_string(),
                "album_artist".to_string(),
            ],
        }
    }
}

/// Opinions for performance tuning (threads, caches, instrumentation)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerformanceOpinions {
    /// Number of worker threads. None = 2x logical cores (default).
    pub worker_threads: Option<usize>,
    /// SQLite page cache size per connection in MB (default: 256).
    pub db_cache_mb: u32,
    /// Enable timing instrumentation and stats display (default: false).
    /// When false, skips all atomic counter updates for better performance.
    pub timing_instrumentation: bool,
}

impl Default for PerformanceOpinions {
    fn default() -> Self {
        Self {
            worker_threads: None, // 2x cores
            db_cache_mb: 256,
            timing_instrumentation: false,
        }
    }
}

/// Opinions for detecting and splitting compound tag values.
///
/// Maps tag names to their separator characters. When a tag value contains
/// any of its configured separators, it's flagged for potential splitting.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagSplittingOpinions {
    /// Map of tag_name -> separators to detect.
    /// Default: { "genre": [";", ",", "/"] }
    pub tag_separators: std::collections::HashMap<String, Vec<String>>,
}

impl Default for TagSplittingOpinions {
    fn default() -> Self {
        let mut tag_separators = std::collections::HashMap::new();
        tag_separators.insert(
            "genre".to_string(),
            vec![";".to_string(), ",".to_string(), "/".to_string()],
        );
        Self { tag_separators }
    }
}

/// Opinions for fingerprint duplicate analysis.
///
/// Controls similarity thresholds and duration tolerance for detecting
/// true duplicates vs. different versions of tracks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DuplicateAnalysisOpinions {
    /// Fingerprint similarity threshold (0-100). Pairs below this are not duplicates.
    /// Default: 85.0
    pub fingerprint_similarity_threshold: f64,
    /// Duration tolerance in milliseconds. Tracks with duration difference above this
    /// are clustered separately. Default: 2000 (2 seconds)
    pub duration_tolerance_ms: i64,
}

impl Default for DuplicateAnalysisOpinions {
    fn default() -> Self {
        Self {
            fingerprint_similarity_threshold: 85.0,
            duration_tolerance_ms: 2000,
        }
    }
}

// =============================================================================
// Global Performance Config
// =============================================================================

static PERFORMANCE_CONFIG: OnceLock<PerformanceOpinions> = OnceLock::new();

/// Initialize the global performance config. Called once at startup.
pub fn init_performance_config(opinions: PerformanceOpinions) {
    let _ = PERFORMANCE_CONFIG.set(opinions);
}

/// Get the configured worker thread count.
/// Returns 2x logical cores if not configured or not initialized.
pub fn get_worker_thread_count() -> usize {
    let default_count = std::thread::available_parallelism()
        .map(|n| n.get() * 2)
        .unwrap_or(8);

    PERFORMANCE_CONFIG
        .get()
        .and_then(|p| p.worker_threads)
        .unwrap_or(default_count)
}

/// Get the configured DB cache size in KB (for SQLite PRAGMA cache_size).
/// Returns negative value as SQLite interprets negative as KB.
pub fn get_db_cache_kb() -> i64 {
    let mb = PERFORMANCE_CONFIG
        .get()
        .map(|p| p.db_cache_mb)
        .unwrap_or(256);

    // Convert MB to KB, return as negative (SQLite convention for KB)
    -(mb as i64 * 1024)
}

/// Check if timing instrumentation is enabled.
/// When false, stats collection is skipped entirely for better performance.
pub fn is_timing_enabled() -> bool {
    PERFORMANCE_CONFIG
        .get()
        .map(|p| p.timing_instrumentation)
        .unwrap_or(false)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployMapping {
    pub corpus_relative_paths: Vec<PathBuf>, // Multiple paths, relative to corpus-root
    pub library_names: Vec<String>,          // Target library names
}

impl Config {
    // =========================================================================
    // Derived directory accessors
    // =========================================================================

    /// Corpus directory: `<root>/corpus/`
    pub fn corpus_dir(&self) -> PathBuf {
        self.root.join("corpus")
    }

    /// Libraries directory: `<root>/libraries/`
    pub fn libraries_dir(&self) -> PathBuf {
        self.root.join("libraries")
    }

    /// Stash directory: `<root>/stash/`
    pub fn stash_dir(&self) -> PathBuf {
        self.root.join("stash")
    }

    /// Legacy library directory: `<root>/libraries/legacy/`
    /// Only meaningful when `legacy_enabled` is true.
    pub fn legacy_dir(&self) -> PathBuf {
        self.libraries_dir().join("legacy")
    }

    // =========================================================================
    // Deploy mapping queries
    // =========================================================================

    /// Get all corpus paths that deploy to a specific library.
    ///
    /// Returns absolute paths to corpus directories that are configured
    /// to deploy to the given library name.
    pub fn get_corpus_paths_for_library(&self, library_name: &str) -> Vec<PathBuf> {
        let corpus_dir = self.corpus_dir();
        let mut paths = Vec::new();
        for mapping in &self.deploy_mappings {
            if mapping.library_names.contains(&library_name.to_string()) {
                for corpus_relative_path in &mapping.corpus_relative_paths {
                    paths.push(corpus_dir.join(corpus_relative_path));
                }
            }
        }
        paths
    }

    /// Check if a file path is configured for deployment.
    ///
    /// Returns true if the path starts with any configured deploy corpus path.
    /// Paths are expected in relative format: `corpus/<relative-path>`.
    pub fn is_path_configured_for_deploy(&self, path: &std::path::Path) -> bool {
        for mapping in &self.deploy_mappings {
            for corpus_relative_path in &mapping.corpus_relative_paths {
                let relative_prefix = std::path::Path::new("corpus").join(corpus_relative_path);
                if path.starts_with(&relative_prefix) {
                    return true;
                }
            }
        }
        false
    }

    /// Get the target library name for a corpus path.
    ///
    /// Given a corpus path (relative, e.g., `corpus/web/releases/...`), returns
    /// the first matching library name from deploy mappings.
    /// Returns None if no mapping matches.
    pub fn get_library_for_corpus_path(&self, corpus_path: &std::path::Path) -> Option<String> {
        for mapping in &self.deploy_mappings {
            for corpus_relative_path in &mapping.corpus_relative_paths {
                let relative_prefix = std::path::Path::new("corpus").join(corpus_relative_path);
                if corpus_path.starts_with(&relative_prefix) {
                    // Return first library name (primary target)
                    return mapping.library_names.first().cloned();
                }
            }
        }
        None
    }

    // =========================================================================
    // Validation
    // =========================================================================

    /// Validate that the archive root exists and all subdirectories are on the same filesystem.
    ///
    /// This check ensures:
    /// - Hard links will work (corpus → libraries)
    /// - Atomic moves will work (corpus → stash)
    ///
    /// Uses st_dev (device ID) comparison rather than creating test files.
    /// Runtime checks during corpus walking will detect nested mount points.
    pub fn validate_same_filesystem(&self) -> Result<()> {
        use std::os::unix::fs::MetadataExt;

        let corpus_dir = self.corpus_dir();
        let libraries_dir = self.libraries_dir();
        let stash_dir = self.stash_dir();

        // Validate root exists
        if !self.root.exists() {
            anyhow::bail!(
                "Validation failed: archive root does not exist\n\
                 Path: {:?}\n\
                 \n\
                 Please create this directory or update config.kdl",
                self.root
            );
        }

        // Validate subdirectories exist and collect device IDs
        let mut device_ids: Vec<(&str, &PathBuf, u64)> = Vec::new();

        // Check root first
        let root_dev = std::fs::metadata(&self.root)
            .with_context(|| format!("Failed to stat root directory: {:?}", self.root))?
            .dev();
        device_ids.push(("root", &self.root, root_dev));

        for (name, dir) in [("corpus", &corpus_dir), ("libraries", &libraries_dir), ("stash", &stash_dir)] {
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
                    first_name, first_dir, first_dev,
                    name, dir, dev
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

    /// Validate deployment configuration.
    pub fn validate(&self) -> Result<()> {
        self.validate_same_filesystem()
            .context("Filesystem validation failed")?;

        // Validate deploy paths don't escape corpus
        let corpus_dir = self.corpus_dir();
        for mapping in &self.deploy_mappings {
            for corpus_path in &mapping.corpus_relative_paths {
                let full_path = corpus_dir.join(corpus_path);
                if !full_path.starts_with(&corpus_dir) {
                    anyhow::bail!("Deploy path escapes corpus directory: {:?}", corpus_path);
                }
            }
        }

        Ok(())
    }
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
            Please create a config file at $XDG_CONFIG_HOME/mla/config.kdl (or ~/.config/mla/config.kdl)",
            config_path
        );
    }

    let content = fs::read_to_string(&config_path)
        .with_context(|| format!("Failed to read config from {:?}", config_path))?;

    parse_kdl_config(&content)
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

/// Parse startup opinions from KDL node
fn parse_startup_opinions(node: &kdl::KdlNode, opinions: &mut StartupOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "freshen-last-stage" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.freshen_last_stage_at_startup = val;
                        }
                    }
                }
                "force-check-all-files" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.force_check_all_files_at_startup = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse health detection opinions from KDL node
fn parse_health_detection_opinions(node: &kdl::KdlNode, opinions: &mut HealthDetectionOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            if child.name().value() == "required-tags" {
                // Collect all string values from the node entries
                let tags: Vec<String> = child
                    .entries()
                    .iter()
                    .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                    .collect();
                if !tags.is_empty() {
                    opinions.required_tags = tags;
                }
            }
        }
    }
}

/// Parse a human-readable size string like "256mb", "1gb", "512" into MB.
/// Accepts: plain numbers (interpreted as MB), or suffixed with kb/mb/gb (case-insensitive).
fn parse_size_mb(s: &str) -> Option<u32> {
    let s = s.trim().to_lowercase();

    if let Some(num_str) = s.strip_suffix("gb") {
        num_str.trim().parse::<u32>().ok().map(|n| n * 1024)
    } else if let Some(num_str) = s.strip_suffix("mb") {
        num_str.trim().parse::<u32>().ok()
    } else if let Some(num_str) = s.strip_suffix("kb") {
        // KB rounds up to nearest MB (minimum 1MB)
        num_str.trim().parse::<u32>().ok().map(|n| (n + 1023) / 1024)
    } else {
        // Plain number = MB
        s.parse::<u32>().ok()
    }
}

/// Parse performance opinions from KDL node
fn parse_performance_opinions(node: &kdl::KdlNode, opinions: &mut PerformanceOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "worker-threads" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.worker_threads = Some(val as usize);
                            }
                        }
                    }
                }
                "db-cache" => {
                    if let Some(entry) = child.entries().first() {
                        // Try string first (e.g., "256mb", "1gb")
                        if let Some(s) = entry.value().as_string() {
                            if let Some(mb) = parse_size_mb(s) {
                                opinions.db_cache_mb = mb;
                            }
                        }
                        // Fall back to plain integer (interpreted as MB)
                        else if let Some(val) = entry.value().as_i64() {
                            if val > 0 {
                                opinions.db_cache_mb = val as u32;
                            }
                        }
                    }
                }
                "timing-instrumentation" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_bool() {
                            opinions.timing_instrumentation = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

/// Parse tag-splitting opinions from KDL node.
///
/// Expected format:
/// ```kdl
/// tag-splitting {
///     "genre" ";" "," "/"
///     "artist" "&" "," ";"
/// }
/// ```
/// Each child node is a tag name, with separator strings as entries.
fn parse_tag_splitting_opinions(node: &kdl::KdlNode, opinions: &mut TagSplittingOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            let tag_name = child.name().value().to_string();
            // Collect all string entries as separators
            let separators: Vec<String> = child
                .entries()
                .iter()
                .filter_map(|e| e.value().as_string().map(|s| s.to_string()))
                .collect();
            if !separators.is_empty() {
                opinions.tag_separators.insert(tag_name, separators);
            }
        }
    }
}

/// Parse duplicate-analysis opinions from KDL node
fn parse_duplicate_analysis_opinions(node: &kdl::KdlNode, opinions: &mut DuplicateAnalysisOpinions) {
    if let Some(children) = node.children() {
        for child in children.nodes() {
            match child.name().value() {
                "fingerprint-similarity-threshold" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_f64() {
                            opinions.fingerprint_similarity_threshold = val;
                        }
                    }
                }
                "duration-tolerance-ms" => {
                    if let Some(entry) = child.entries().first() {
                        if let Some(val) = entry.value().as_i64() {
                            opinions.duration_tolerance_ms = val;
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn parse_kdl_config(content: &str) -> Result<Config> {
    let doc: kdl::KdlDocument = content.parse().context("Failed to parse KDL document")?;

    let mut config = Config {
        root: PathBuf::new(),
        legacy_enabled: false,
        deploy_mappings: Vec::new(),
        opinions: Opinions::default(),
    };

    for node in doc.nodes() {
        match node.name().value() {
            "root" => {
                if let Some(path) = node.entries().first() {
                    if let Some(path_str) = path.value().as_string() {
                        config.root = PathBuf::from(path_str);
                    }
                }
            }
            "legacy-library" => {
                // Boolean toggle: `legacy-library true`
                if let Some(entry) = node.entries().first() {
                    if let Some(val) = entry.value().as_bool() {
                        config.legacy_enabled = val;
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
                            "startup" => {
                                parse_startup_opinions(child, &mut config.opinions.startup);
                            }
                            "health-detection" => {
                                parse_health_detection_opinions(child, &mut config.opinions.health_detection);
                            }
                            "performance" => {
                                parse_performance_opinions(child, &mut config.opinions.performance);
                            }
                            "tag-splitting" => {
                                parse_tag_splitting_opinions(child, &mut config.opinions.tag_splitting);
                            }
                            "duplicate-analysis" => {
                                parse_duplicate_analysis_opinions(child, &mut config.opinions.duplicate_analysis);
                            }
                            _ => {}
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if config.root.as_os_str().is_empty() {
        anyhow::bail!("root not specified in config.kdl");
    }

    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_single_root() {
        let kdl = r#"
root "/Volumes/cerberus/archive"

deploy "web/releases/bandcamp" "web/releases/itunes" {
    library "main"
}

legacy-library true
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert_eq!(config.root, PathBuf::from("/Volumes/cerberus/archive"));
        assert_eq!(config.corpus_dir(), PathBuf::from("/Volumes/cerberus/archive/corpus"));
        assert_eq!(config.libraries_dir(), PathBuf::from("/Volumes/cerberus/archive/libraries"));
        assert_eq!(config.stash_dir(), PathBuf::from("/Volumes/cerberus/archive/stash"));
        assert_eq!(config.legacy_dir(), PathBuf::from("/Volumes/cerberus/archive/libraries/legacy"));
        assert!(config.legacy_enabled);
        assert_eq!(config.deploy_mappings.len(), 1);
        assert_eq!(config.deploy_mappings[0].corpus_relative_paths.len(), 2);
        assert_eq!(config.deploy_mappings[0].library_names[0], "main");
    }

    #[test]
    fn test_missing_root() {
        let kdl = r#"
legacy-library true
"#;

        let result = parse_kdl_config(kdl);
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(err_msg.contains("root not specified"));
    }

    #[test]
    fn test_legacy_disabled_by_default() {
        let kdl = r#"
root "/archive"
"#;

        let config = parse_kdl_config(kdl).unwrap();
        assert!(!config.legacy_enabled);
    }

    #[test]
    fn test_opinions_parsing() {
        let kdl = r#"
root "/archive"

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

        assert!(config.opinions.auto_next_save_all);
        assert_eq!(config.opinions.fingerprint_matching.duration_tolerance_percent, 15.0);
        assert!(!config.opinions.fingerprint_matching.require_matching_track_number);
        assert!(config.opinions.fingerprint_matching.require_matching_album);
        assert!(!config.opinions.quality_resolution.auto_resolve_format_tier);
        assert_eq!(config.opinions.quality_resolution.bitrate_threshold_percent, 75.0);
        assert!(!config.opinions.canonicalization.case_insensitive);
        assert!(config.opinions.canonicalization.strip_parentheticals);
        assert_eq!(config.opinions.canonicalization.fuzzy_threshold, 0.90);
    }

    #[test]
    fn test_opinions_defaults() {
        let kdl = r#"
root "/archive"
"#;

        let config = parse_kdl_config(kdl).unwrap();

        assert!(!config.opinions.auto_next_save_all);
        assert_eq!(config.opinions.fingerprint_matching.duration_tolerance_percent, 10.0);
        assert!(config.opinions.fingerprint_matching.require_matching_track_number);
        assert!(!config.opinions.fingerprint_matching.require_matching_album);
        assert!(config.opinions.quality_resolution.auto_resolve_format_tier);
        assert_eq!(config.opinions.quality_resolution.bitrate_threshold_percent, 50.0);
        assert!(config.opinions.canonicalization.case_insensitive);
        assert!(!config.opinions.canonicalization.strip_parentheticals);
        assert_eq!(config.opinions.canonicalization.fuzzy_threshold, 0.85);
    }

    #[test]
    fn test_is_path_configured_for_deploy() {
        let kdl = r#"
root "/archive"

deploy "web/releases/bandcamp" {
    library "music"
}

deploy "web/releases/steam" {
    library "soundtracks"
}
"#;

        let config = parse_kdl_config(kdl).unwrap();

        // Relative paths (as stored in DB) should match
        assert!(config.is_path_configured_for_deploy(std::path::Path::new(
            "corpus/web/releases/bandcamp/Artist/Album/track.flac"
        )));
        assert!(config.is_path_configured_for_deploy(std::path::Path::new(
            "corpus/web/releases/steam/Game/Soundtrack/01.mp3"
        )));

        // Non-configured paths should not match
        assert!(!config.is_path_configured_for_deploy(std::path::Path::new(
            "corpus/web/releases/itunes/Artist/Album/track.flac"
        )));
        assert!(!config.is_path_configured_for_deploy(std::path::Path::new(
            "corpus/physical/cd/Artist/Album/track.flac"
        )));

        // Exact prefix match (not substring)
        assert!(!config.is_path_configured_for_deploy(std::path::Path::new(
            "corpus/web/releases/bandcamp-extra/Artist/track.flac"
        )));
    }
}
