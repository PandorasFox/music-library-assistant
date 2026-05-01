//! Environment variable overrides for MM configuration.
//!
//! Called after KDL parsing to layer environment values on top of file-based config.
//! Two mechanisms:
//!
//! 1. **Curated aliases**: `MM_ROOT`, `MM_ACOUSTID_API_KEY`, etc. — direct mappings
//!    to common config fields.
//! 2. **Generic fallback**: `MM_CFG__<BLOCK>__<FIELD>` maps to `opinions.<block>.<field>`
//!    with SCREAMING_SNAKE → kebab-case conversion. Only flat scalar fields.

use std::env;
use std::path::PathBuf;

use crate::config::parse::parse_size_mb;
use crate::config::types::Config;
use crate::logging;

/// Apply all environment variable overrides to a parsed config.
pub fn apply_env_overrides(config: &mut Config) {
    apply_curated(config);
    apply_generic(config);
}

/// Direct shorthand aliases for the most common config fields.
fn apply_curated(config: &mut Config) {
    if let Ok(val) = env::var("MM_STORAGE_ROOT") {
        logging::log_general(format!("env override: MM_STORAGE_ROOT = {val}"));
        config.storage_root = PathBuf::from(val);
    } else if let Ok(val) = env::var("MM_ROOT") {
        logging::log_general(format!("env override: MM_ROOT = {val} (deprecated, use MM_STORAGE_ROOT)"));
        config.storage_root = PathBuf::from(val);
    }

    if let Ok(val) = env::var("MM_LIBRARIES_ROOT") {
        logging::log_general(format!("env override: MM_LIBRARIES_ROOT = {val}"));
        config.libraries_root = Some(PathBuf::from(val));
    }

    if let Ok(val) = env::var("MM_STASH_ROOT") {
        logging::log_general(format!("env override: MM_STASH_ROOT = {val}"));
        config.stash_root = Some(PathBuf::from(val));
    }

    if let Ok(val) = env::var("MM_ACOUSTID_API_KEY") {
        logging::log_general("env override: MM_ACOUSTID_API_KEY = <redacted>");
        config.opinions.external_matching.acoustid_api_key = val;
    }

    if let Ok(val) = env::var("MM_MB_BASE_URL") {
        logging::log_general(format!("env override: MM_MB_BASE_URL = {val}"));
        config.opinions.external_matching.mb_base_url = val;
    }

    if let Ok(val) = env::var("MM_MB_REQUESTS_PER_SECOND") {
        if let Ok(n) = val.parse::<u32>() {
            logging::log_general(format!("env override: MM_MB_REQUESTS_PER_SECOND = {n}"));
            config.opinions.external_matching.mb_requests_per_second = n;
        } else {
            logging::log_general(format!(
                "env override: MM_MB_REQUESTS_PER_SECOND invalid u32: {val}"
            ));
        }
    }

    if let Ok(val) = env::var("MM_WORKER_THREADS") {
        if let Ok(n) = val.parse::<usize>() {
            logging::log_general(format!("env override: MM_WORKER_THREADS = {n}"));
            config.opinions.performance.worker_threads = Some(n);
        } else {
            logging::log_general(format!(
                "env override: MM_WORKER_THREADS invalid usize: {val}"
            ));
        }
    }

    if let Ok(val) = env::var("MM_DB_CACHE") {
        if let Some(mb) = parse_size_mb(&val) {
            logging::log_general(format!("env override: MM_DB_CACHE = {mb}mb"));
            config.opinions.performance.db_cache_mb = mb;
        } else {
            logging::log_general(format!("env override: MM_DB_CACHE invalid size: {val}"));
        }
    }

    if let Ok(val) = env::var("MM_WATCHER_POLL_INTERVAL") {
        if let Some(secs) = parse_duration_secs(&val) {
            logging::log_general(format!(
                "env override: MM_WATCHER_POLL_INTERVAL = {secs}s"
            ));
            config.opinions.watcher_poll_interval_secs = secs;
        } else {
            logging::log_general(format!(
                "env override: MM_WATCHER_POLL_INTERVAL invalid duration: {val}"
            ));
        }
    }

    if let Ok(val) = env::var("MM_AUTO_DEPLOY") {
        match val.to_lowercase().as_str() {
            "true" | "1" | "yes" => {
                logging::log_general("env override: MM_AUTO_DEPLOY = true");
                config.opinions.auto_deploy = true;
            }
            "false" | "0" | "no" => {
                logging::log_general("env override: MM_AUTO_DEPLOY = false");
                config.opinions.auto_deploy = false;
            }
            _ => {
                logging::log_general(format!(
                    "env override: MM_AUTO_DEPLOY invalid bool: {val}"
                ));
            }
        }
    }
}

/// Scan `MM_CFG__*` env vars and apply them as opinion overrides.
///
/// Convention: `MM_CFG__<BLOCK>__<FIELD>` → `opinions.<block>.<field>`
/// where SCREAMING_SNAKE names are lowercased and underscores become hyphens.
fn apply_generic(config: &mut Config) {
    let mut vars: Vec<(String, String)> = env::vars()
        .filter(|(k, _)| k.starts_with("MM_CFG__"))
        .collect();
    // Sort for deterministic application order
    vars.sort_by(|a, b| a.0.cmp(&b.0));

    for (key, val) in vars {
        let rest = &key["MM_CFG__".len()..];
        let parts: Vec<&str> = rest.splitn(3, "__").collect();
        if parts.len() != 2 {
            logging::log_general(format!(
                "env override: {key} skipped — expected MM_CFG__BLOCK__FIELD"
            ));
            continue;
        }

        let block = screaming_to_kebab(parts[0]);
        let field = screaming_to_kebab(parts[1]);

        if !apply_generic_field(config, &block, &field, &val) {
            logging::log_general(format!(
                "env override: {key} skipped — unknown path opinions.{block}.{field}"
            ));
        } else {
            logging::log_general(format!(
                "env override: {key} → opinions.{block}.{field} = {val}"
            ));
        }
    }
}

/// Convert SCREAMING_SNAKE_CASE to kebab-case.
fn screaming_to_kebab(s: &str) -> String {
    s.to_lowercase().replace('_', "-")
}

/// Try to apply a generic env var to a known opinions field.
/// Returns true if the (block, field) path was recognized and applied.
fn apply_generic_field(config: &mut Config, block: &str, field: &str, val: &str) -> bool {
    use crate::config::types::*;

    match (block, field) {
        // startup
        (Opinions::KDL_BLOCK_STARTUP, StartupOpinions::KDL_FORCE_CHECK) => {
            if let Ok(b) = parse_bool(val) {
                config.opinions.startup.force_check_all_files_at_startup = b;
                return true;
            }
        }
        (Opinions::KDL_BLOCK_STARTUP, StartupOpinions::KDL_VACUUM_THRESHOLD) => {
            if let Ok(f) = val.parse::<f64>() {
                config.opinions.startup.vacuum_threshold = f;
                return true;
            }
        }

        // canonicalization
        (
            Opinions::KDL_BLOCK_CANONICALIZATION,
            CanonicalizationOpinions::KDL_STRIP_SUFFIXES,
        ) => {
            if let Ok(b) = parse_bool(val) {
                config.opinions.canonicalization.strip_album_format_suffixes = b;
                return true;
            }
        }

        // health-detection (only scalar fields — required_tags is a list, skip it)
        (
            Opinions::KDL_BLOCK_HEALTH_DETECTION,
            HealthDetectionOpinions::KDL_ALBUM_ARTIST_COMPILATION,
        ) => {
            if let Ok(b) = parse_bool(val) {
                config
                    .opinions
                    .health_detection
                    .album_artist_only_required_if_compilation = b;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_HEALTH_DETECTION,
            HealthDetectionOpinions::KDL_SINGLE_ALBUM_SUFFIX,
        ) => {
            config.opinions.health_detection.single_album_suffix = val.to_string();
            return true;
        }

        // performance
        (Opinions::KDL_BLOCK_PERFORMANCE, PerformanceOpinions::KDL_WORKER_THREADS) => {
            if let Ok(n) = val.parse::<usize>() {
                config.opinions.performance.worker_threads = Some(n);
                return true;
            }
        }
        (Opinions::KDL_BLOCK_PERFORMANCE, PerformanceOpinions::KDL_DB_CACHE) => {
            if let Some(mb) = parse_size_mb(val) {
                config.opinions.performance.db_cache_mb = mb;
                return true;
            }
        }

        // duplicate-analysis
        (
            Opinions::KDL_BLOCK_DUPLICATE_ANALYSIS,
            DuplicateAnalysisOpinions::KDL_FP_THRESHOLD,
        ) => {
            if let Ok(f) = val.parse::<f64>() {
                config
                    .opinions
                    .duplicate_analysis
                    .fingerprint_similarity_threshold = f;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_DUPLICATE_ANALYSIS,
            DuplicateAnalysisOpinions::KDL_DURATION_TOLERANCE,
        ) => {
            if let Ok(n) = val.parse::<i64>() {
                config.opinions.duplicate_analysis.duration_tolerance_ms = n;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_DUPLICATE_ANALYSIS,
            DuplicateAnalysisOpinions::KDL_ELIDE_VARIANTS,
        ) => {
            if let Ok(b) = parse_bool(val) {
                config.opinions.duplicate_analysis.elide_variant_titles = b;
                return true;
            }
        }

        // release-packing (scalar fields only — weights are sub-blocks, skip)
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_DURATION_TOLERANCE_PCT,
        ) => {
            if let Ok(f) = val.parse::<f64>() {
                config.opinions.release_packing.duration_tolerance_pct = f;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_MIN_CONFIDENCE,
        ) => {
            if let Ok(f) = val.parse::<f64>() {
                config.opinions.release_packing.min_confidence = f;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_TITLE_PREASSIGN_THRESHOLD,
        ) => {
            if let Ok(f) = val.parse::<f64>() {
                config.opinions.release_packing.title_preassign_threshold = f;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_PACKING_KNOT_RATIO,
        ) => {
            if let Ok(f) = val.parse::<f64>() {
                config.opinions.release_packing.packing_knot_ratio = f;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_PACKING_KNOT_SIZE_LIMIT,
        ) => {
            if let Ok(n) = val.parse::<usize>() {
                config.opinions.release_packing.packing_knot_size_limit = n;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_SINGLES_BEFORE_INCOMPLETES,
        ) => {
            if let Ok(b) = parse_bool(val) {
                config.opinions.release_packing.singles_before_incompletes = b;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_ALLOW_DISCOGRAPHY_REDUCTION,
        ) => {
            if let Ok(b) = parse_bool(val) {
                config
                    .opinions
                    .release_packing
                    .allow_resolve_knots_with_discographies = b;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ACOUSTID_RATIO,
        ) => {
            if let Ok(f) = val.parse::<f64>() {
                config
                    .opinions
                    .release_packing
                    .low_confidence_max_acoustid_ratio = f;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_LOW_CONFIDENCE_ALBUM_MATCH,
        ) => {
            if let Ok(f) = val.parse::<f64>() {
                config
                    .opinions
                    .release_packing
                    .low_confidence_max_album_match = f;
                return true;
            }
        }
        (
            Opinions::KDL_BLOCK_RELEASE_PACKING,
            ReleasePackingOpinions::KDL_IDLE_FULL_REPACK_AFTER_SECS,
        ) => {
            if let Ok(n) = val.parse::<u64>() {
                config.opinions.release_packing.idle_full_repack_after_secs = n;
                return true;
            }
        }

        // external-matching
        (Opinions::KDL_BLOCK_EXTERNAL_MATCHING, ExternalMatchingConfig::KDL_ACOUSTID_KEY) => {
            config.opinions.external_matching.acoustid_api_key = val.to_string();
            return true;
        }
        (Opinions::KDL_BLOCK_EXTERNAL_MATCHING, ExternalMatchingConfig::KDL_REQ_PER_SEC) => {
            if let Ok(n) = val.parse::<u32>() {
                config.opinions.external_matching.requests_per_second = n;
                return true;
            }
        }
        (Opinions::KDL_BLOCK_EXTERNAL_MATCHING, ExternalMatchingConfig::KDL_MB_REQ_PER_SEC) => {
            if let Ok(n) = val.parse::<u32>() {
                config.opinions.external_matching.mb_requests_per_second = n;
                return true;
            }
        }
        (Opinions::KDL_BLOCK_EXTERNAL_MATCHING, ExternalMatchingConfig::KDL_MB_BASE_URL) => {
            config.opinions.external_matching.mb_base_url = val.to_string();
            return true;
        }
        (Opinions::KDL_BLOCK_EXTERNAL_MATCHING, ExternalMatchingConfig::KDL_AUTO_ENRICH) => {
            if let Ok(b) = parse_bool(val) {
                config.opinions.external_matching.auto_enrich_on_match = b;
                return true;
            }
        }
        (Opinions::KDL_BLOCK_EXTERNAL_MATCHING, ExternalMatchingConfig::KDL_MB_CACHE_TTL) => {
            if let Ok(n) = val.parse::<u32>() {
                config.opinions.external_matching.mb_cache_ttl_days = n;
                return true;
            }
        }
        // cover_art_types is a list — not supported via generic env override

        // disc-extraction
        (Opinions::KDL_BLOCK_DISC_EXTRACTION, DiscExtractionOpinions::KDL_DISC_TAG_NAME) => {
            config.opinions.disc_extraction.disc_tag_name = val.to_string();
            return true;
        }
        (Opinions::KDL_BLOCK_DISC_EXTRACTION, DiscExtractionOpinions::KDL_MAP_LETTERS) => {
            if let Ok(b) = parse_bool(val) {
                config.opinions.disc_extraction.map_letters_to_numbers = b;
                return true;
            }
        }

        // album-art
        (Opinions::KDL_BLOCK_ALBUM_ART, AlbumArtOpinions::KDL_SIDECAR_DEPLOY) => {
            match val.to_lowercase().as_str() {
                "disabled" => {
                    config.opinions.album_art.sidecar_deploy_mode = SidecarDeployMode::Disabled;
                    return true;
                }
                "primarycover" | "primary-cover" => {
                    config.opinions.album_art.sidecar_deploy_mode =
                        SidecarDeployMode::PrimaryCover;
                    return true;
                }
                "all" => {
                    config.opinions.album_art.sidecar_deploy_mode = SidecarDeployMode::All;
                    return true;
                }
                _ => {}
            }
        }

        _ => {}
    }

    false
}

/// Parse a string as a boolean (case-insensitive).
fn parse_bool(s: &str) -> Result<bool, ()> {
    match s.to_lowercase().as_str() {
        "true" | "1" | "yes" => Ok(true),
        "false" | "0" | "no" => Ok(false),
        _ => Err(()),
    }
}

/// Parse a duration string as seconds.
/// Accepts plain integer (seconds) or humantime-style suffixes: "30s", "5m", "1h", "15min".
fn parse_duration_secs(s: &str) -> Option<u64> {
    let s = s.trim().to_lowercase();

    if let Some(rest) = s.strip_suffix('h') {
        return rest.trim().parse::<u64>().ok().map(|n| n * 3600);
    }
    if let Some(rest) = s.strip_suffix("min") {
        return rest.trim().parse::<u64>().ok().map(|n| n * 60);
    }
    if let Some(rest) = s.strip_suffix('m') {
        return rest.trim().parse::<u64>().ok().map(|n| n * 60);
    }
    if let Some(rest) = s.strip_suffix('s') {
        return rest.trim().parse::<u64>().ok();
    }
    s.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::types::{Config, Opinions};
    use mm_utils::t;
    use std::path::PathBuf;
    use std::sync::Mutex;

    /// Env vars are process-global. Tests that call set_var + apply_env_overrides
    /// + remove_var race when run in parallel. This mutex serializes them.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    /// Build a minimal Config for testing (no filesystem needed).
    fn test_config() -> Config {
        Config {
            storage_root: PathBuf::from("/original/root"),
            libraries_root: None,
            stash_root: None,
            source_dirs: Vec::new(),
            opinions: Opinions::default(),
        }
    }

    #[test]
    fn curated_mm_root_overrides_config() {
        let _guard = t!(ENV_LOCK.lock());
        let mut config = test_config();
        assert_eq!(config.storage_root, PathBuf::from("/original/root"));

        env::set_var("MM_ROOT", "/overridden/root");
        apply_env_overrides(&mut config);
        env::remove_var("MM_ROOT");

        assert_eq!(config.storage_root, PathBuf::from("/overridden/root"));
    }

    #[test]
    fn curated_mm_worker_threads_overrides_config() {
        let _guard = t!(ENV_LOCK.lock());
        let mut config = test_config();
        assert_eq!(config.opinions.performance.worker_threads, None);

        env::set_var("MM_WORKER_THREADS", "42");
        apply_env_overrides(&mut config);
        env::remove_var("MM_WORKER_THREADS");

        assert_eq!(config.opinions.performance.worker_threads, Some(42));
    }

    #[test]
    fn curated_mm_db_cache_accepts_size_string() {
        let _guard = t!(ENV_LOCK.lock());
        let mut config = test_config();

        env::set_var("MM_DB_CACHE", "1gb");
        apply_env_overrides(&mut config);
        env::remove_var("MM_DB_CACHE");

        assert_eq!(config.opinions.performance.db_cache_mb, 1024);
    }

    #[test]
    fn curated_mm_watcher_poll_interval_accepts_humantime() {
        let _guard = t!(ENV_LOCK.lock());
        let mut config = test_config();

        env::set_var("MM_WATCHER_POLL_INTERVAL", "5m");
        apply_env_overrides(&mut config);
        env::remove_var("MM_WATCHER_POLL_INTERVAL");

        assert_eq!(config.opinions.watcher_poll_interval_secs, 300);
    }

    #[test]
    fn generic_mm_cfg_overrides_config() {
        let _guard = t!(ENV_LOCK.lock());
        let mut config = test_config();
        assert!((config.opinions.duplicate_analysis.fingerprint_similarity_threshold - 95.0).abs() < f64::EPSILON);

        env::set_var("MM_CFG__DUPLICATE_ANALYSIS__FINGERPRINT_SIMILARITY_THRESHOLD", "90.0");
        apply_env_overrides(&mut config);
        env::remove_var("MM_CFG__DUPLICATE_ANALYSIS__FINGERPRINT_SIMILARITY_THRESHOLD");

        assert!((config.opinions.duplicate_analysis.fingerprint_similarity_threshold - 90.0).abs() < f64::EPSILON);
    }

    #[test]
    fn generic_unknown_field_does_not_panic() {
        let _guard = t!(ENV_LOCK.lock());
        let mut config = test_config();
        let original = config.clone();

        env::set_var("MM_CFG__NONEXISTENT__FAKE_FIELD", "whatever");
        apply_env_overrides(&mut config);
        env::remove_var("MM_CFG__NONEXISTENT__FAKE_FIELD");

        // Config unchanged (no curated vars leaked from other tests)
        assert_eq!(config.storage_root, original.storage_root);
    }

    #[test]
    fn screaming_to_kebab_conversion() {
        assert_eq!(screaming_to_kebab("DUPLICATE_ANALYSIS"), "duplicate-analysis");
        assert_eq!(screaming_to_kebab("MB_BASE_URL"), "mb-base-url");
        assert_eq!(screaming_to_kebab("PERFORMANCE"), "performance");
    }

    #[test]
    fn parse_bool_variants() {
        assert_eq!(parse_bool("true"), Ok(true));
        assert_eq!(parse_bool("TRUE"), Ok(true));
        assert_eq!(parse_bool("1"), Ok(true));
        assert_eq!(parse_bool("yes"), Ok(true));
        assert_eq!(parse_bool("false"), Ok(false));
        assert_eq!(parse_bool("0"), Ok(false));
        assert_eq!(parse_bool("no"), Ok(false));
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn parse_duration_secs_variants() {
        assert_eq!(parse_duration_secs("300"), Some(300));
        assert_eq!(parse_duration_secs("5m"), Some(300));
        assert_eq!(parse_duration_secs("15min"), Some(900));
        assert_eq!(parse_duration_secs("1h"), Some(3600));
        assert_eq!(parse_duration_secs("30s"), Some(30));
        assert_eq!(parse_duration_secs("garbage"), None);
    }
}
