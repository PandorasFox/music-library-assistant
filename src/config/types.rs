//! Configuration types: structs, enums, Default impls, and Config methods.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock, RwLockReadGuard};

use super::path_schema::PathTagSchema;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub root: PathBuf,
    pub legacy_enabled: bool,
    pub source_dirs: Vec<SourceDir>,
    pub opinions: Opinions,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Opinions {
    /// When true, lossy shit formats (MP3, M4A, etc.) are captured to FLAC
    /// instead of transcoded to Opus. The decoded PCM waveform is losslessly
    /// stored in a FLAC container with extension `.mp3.LOSSY.flac`.
    pub lossy_shit_formats_to_flac: bool,
    pub quality_resolution: QualityResolutionOpinions,
    pub canonicalization: CanonicalizationOpinions,
    pub startup: StartupOpinions,
    pub health_detection: HealthDetectionOpinions,
    pub performance: PerformanceOpinions,
    pub tag_splitting: TagSplittingOpinions,
    pub duplicate_analysis: DuplicateAnalysisOpinions,
    pub release_packing: ReleasePackingOpinions,
    pub inbox_organize: InboxOrganizeOpinions,
    /// Seconds of idle time before auto-rescanning corpus/inbox for filesystem changes.
    /// Default: 180. Set to 0 to disable.
    pub idle_rescan_interval_secs: u64,
    /// When true, keep one persistent transaction open across modal interactions.
    /// Decisions accumulate in a Transaction tab; commit/discard from there.
    /// Default: false.
    pub leave_transactions_open: bool,
    /// External matching (AcoustID, etc.) configuration.
    pub external_matching: ExternalMatchingConfig,
    /// Disc extraction configuration (tag name, letter mapping).
    pub disc_extraction: DiscExtractionOpinions,
    /// Album art embedding/upgrade configuration.
    pub album_art: AlbumArtOpinions,
    /// Debug/diagnostic options.
    pub debug: DebugOpinions,
}

/// KDL field names — single source of truth for parse/edit/source-detection.
impl Opinions {
    // Direct children of the "opinions" block
    pub const KDL_LOSSY_SHIT: &str = "lossy-shit-formats-to-flac";
    pub const KDL_IDLE_RESCAN: &str = "idle-rescan-interval";
    pub const KDL_LEAVE_TXN_OPEN: &str = "leave-transactions-open";

    // Sub-block names
    pub const KDL_BLOCK_STARTUP: &str = "startup";
    pub const KDL_BLOCK_QUALITY_RESOLUTION: &str = "quality-resolution";
    pub const KDL_BLOCK_CANONICALIZATION: &str = "canonicalization";
    pub const KDL_BLOCK_HEALTH_DETECTION: &str = "health-detection";
    pub const KDL_BLOCK_PERFORMANCE: &str = "performance";
    pub const KDL_BLOCK_TAG_SPLITTING: &str = "tag-splitting";
    pub const KDL_BLOCK_DUPLICATE_ANALYSIS: &str = "duplicate-analysis";
    pub const KDL_BLOCK_RELEASE_PACKING: &str = "release-packing";
    pub const KDL_BLOCK_INBOX_ORGANIZE: &str = "inbox-organize";
    pub const KDL_BLOCK_EXTERNAL_MATCHING: &str = "external-matching";
    pub const KDL_BLOCK_DISC_EXTRACTION: &str = "disc-extraction";
    pub const KDL_BLOCK_ALBUM_ART: &str = "album-art";
    pub const KDL_BLOCK_DEBUG: &str = "debug";
}

/// Opinions for quality-based auto-resolution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityResolutionOpinions {
    /// Inbox-to-corpus bitrate fuzz tolerance as a percentage (default: 5.0).
    /// Files within this % bitrate difference (same format class and sample rate)
    /// are treated as equivalent rather than superior/inferior. Suppresses noise
    /// from minor FLAC compression differences across encoder versions.
    pub inbox_bitrate_fuzz_percent: f64,
}

impl Default for QualityResolutionOpinions {
    fn default() -> Self {
        Self {
            inbox_bitrate_fuzz_percent: 5.0,
        }
    }
}

impl QualityResolutionOpinions {
    pub const KDL_BITRATE_FUZZ: &str = "inbox-bitrate-fuzz-percent";
}

/// Opinions for tag canonicalization
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CanonicalizationOpinions {
    /// Strip EP/LP suffixes during album collision detection (default: false).
    /// When true, "Album EP" and "Album" normalize to the same key and collide.
    pub strip_album_format_suffixes: bool,
}

impl CanonicalizationOpinions {
    pub const KDL_STRIP_SUFFIXES: &str = "strip-album-format-suffixes";
}

/// Which view to land on after startup progress completes.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum StartupView {
    #[default]
    Health,
    Search,
    Browser,
    Inbox,
    ExternalMatches,
}

/// Opinions for startup behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartupOpinions {
    /// Force verification of all indexed files at startup, bypassing mtime optimization (default: false).
    /// Catches out-of-band tag changes (external tools modified tags) and corrupt files.
    /// Slower startup but ensures database matches reality.
    pub force_check_all_files_at_startup: bool,
    /// Free-page ratio threshold for prompting DB compaction (default: 0.1 = 10%).
    /// Set to 0.0 to disable.
    pub vacuum_threshold: f64,
    /// Which view to open after startup progress completes (default: Health).
    pub default_view: StartupView,
}

impl Default for StartupOpinions {
    fn default() -> Self {
        Self {
            force_check_all_files_at_startup: false,
            vacuum_threshold: 0.1,
            default_view: StartupView::default(),
        }
    }
}

impl StartupOpinions {
    pub const KDL_FORCE_CHECK: &str = "force-check-all-files";
    pub const KDL_VACUUM_THRESHOLD: &str = "vacuum-threshold";
    pub const KDL_DEFAULT_VIEW: &str = "default-view";
}

/// Opinions for health detection behavior
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthDetectionOpinions {
    /// Tags that must be present on every track (default: title, album, artist, album_artist)
    pub required_tags: Vec<String>,
    /// When true, album_artist is only required on compilation albums (>1 distinct artist).
    /// Single-artist albums without album_artist won't be flagged. Default: true.
    pub album_artist_only_required_if_compilation: bool,
    /// Suffix appended to track title when tagging as single (default: " (Single)").
    pub single_album_suffix: String,
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
            album_artist_only_required_if_compilation: true,
            single_album_suffix: String::new(),
        }
    }
}

impl HealthDetectionOpinions {
    pub const KDL_REQUIRED_TAGS: &str = "required-tags";
    pub const KDL_ALBUM_ARTIST_COMPILATION: &str = "album-artist-only-required-if-compilation";
    pub const KDL_SINGLE_ALBUM_SUFFIX: &str = "single-album-suffix";
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

impl PerformanceOpinions {
    pub const KDL_WORKER_THREADS: &str = "worker-threads";
    pub const KDL_DB_CACHE: &str = "db-cache";
    pub const KDL_TIMING: &str = "timing-instrumentation";
}

/// Opinions for detecting and splitting compound tag values.
///
/// - `collaboration_keywords`: Keywords like "feat", "ft", "vs" for artist collabs
/// - `tag_separators`: Per-tag separator lists (e.g., ARTIST: [";"], GENRE: [";", ","])
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TagSplittingOpinions {
    /// Collaboration keywords for artist tags (e.g., "feat", "ft", "featuring", "vs", "with").
    /// Used to detect featuring patterns like "Artist A feat. Artist B".
    pub collaboration_keywords: std::collections::HashSet<String>,

    /// Per-tag separator strings. Key is uppercase tag name (e.g., "ARTIST", "GENRE").
    /// Each tag has a list of separators to check in order (e.g., [";", ","]).
    pub tag_separators: std::collections::HashMap<String, Vec<String>>,
}

impl Default for TagSplittingOpinions {
    fn default() -> Self {
        let mut collaboration_keywords = std::collections::HashSet::new();
        collaboration_keywords.insert("feat".to_string());
        collaboration_keywords.insert("featuring".to_string());
        collaboration_keywords.insert("ft".to_string());
        collaboration_keywords.insert("with".to_string());
        collaboration_keywords.insert("vs".to_string());

        let mut tag_separators = std::collections::HashMap::new();
        tag_separators.insert("ARTIST".to_string(), vec![";".to_string()]);
        tag_separators.insert("GENRE".to_string(), vec![";".to_string()]);

        Self {
            collaboration_keywords,
            tag_separators,
        }
    }
}

impl TagSplittingOpinions {
    pub const KDL_COLLAB: &str = "collab";
}

/// Opinions for fingerprint duplicate analysis.
///
/// Controls similarity thresholds and duration tolerance for detecting
/// true duplicates vs. different versions of tracks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DuplicateAnalysisOpinions {
    /// Fingerprint similarity threshold (0-100). Pairs below this are not duplicates.
    /// Default: 95.0
    pub fingerprint_similarity_threshold: f64,
    /// Duration tolerance in milliseconds. Tracks with duration difference above this
    /// are clustered separately. Default: 2000 (2 seconds)
    pub duration_tolerance_ms: i64,
    /// Skip duplicate pairs where either title contains variant keywords (remix, live,
    /// acoustic, etc.) and the titles differ. Prevents false duplicate matches between
    /// different versions of the same track. Default: true
    pub elide_variant_titles: bool,
}

impl Default for DuplicateAnalysisOpinions {
    fn default() -> Self {
        Self {
            fingerprint_similarity_threshold: 95.0,
            duration_tolerance_ms: 2000,
            elide_variant_titles: true,
        }
    }
}

impl DuplicateAnalysisOpinions {
    pub const KDL_FP_THRESHOLD: &str = "fingerprint-similarity-threshold";
    pub const KDL_DURATION_TOLERANCE: &str = "duration-tolerance-ms";
    pub const KDL_ELIDE_VARIANTS: &str = "elide-variant-titles";
}

/// Scoring dimension weights for release bin-packing assignment.
///
/// Controls how much each signal dimension contributes to the composite score
/// used by the Hungarian assignment algorithm. Two weight sets exist:
/// `candidate_weights` for AcoustID-backed scoring and `elimination_weights`
/// for tag-only gap-filling where no fingerprint match exists.
///
/// Each tag dimension (title, artist, album) is an independent weight rather
/// than being folded into a single `tag_similarity` composite. This lets
/// elimination scoring zero out `artist_match` when rip artist tags diverge
/// from MusicBrainz credits (common in game soundtracks).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PackingWeights {
    /// AcoustID fingerprint confidence weight.
    pub acoustid_confidence: f64,
    /// Duration match quality weight.
    pub duration_match: f64,
    /// Title similarity weight (max of track title and recording title).
    pub title_match: f64,
    /// Artist similarity weight (corpus ARTIST vs release artist).
    pub artist_match: f64,
    /// Album similarity weight (corpus ALBUM vs release title).
    pub album_match: f64,
    /// Track number match weight.
    pub track_number_match: f64,
}

impl PackingWeights {
    /// Default weights for AcoustID-backed candidate scoring.
    /// Fingerprint confidence and duration are strong anchors.
    pub fn candidate_defaults() -> Self {
        Self {
            acoustid_confidence: 0.30,
            duration_match: 0.30,
            title_match: 0.10,
            artist_match: 0.05,
            album_match: 0.05,
            track_number_match: 0.20,
        }
    }

    /// Default weights for elimination/gap-filling scoring.
    /// No fingerprint available; title and tracknumber are primary evidence.
    /// Artist weight is zero because rip artist tags often diverge from MB
    /// release-level credits (e.g., individual composers vs game studio).
    pub fn elimination_defaults() -> Self {
        Self {
            acoustid_confidence: 0.0,
            duration_match: 0.25,
            title_match: 0.30,
            artist_match: 0.0,
            album_match: 0.05,
            track_number_match: 0.40,
        }
    }

    pub const KDL_ACOUSTID_CONFIDENCE: &str = "acoustid-confidence";
    pub const KDL_DURATION_MATCH: &str = "duration-match";
    pub const KDL_TITLE_MATCH: &str = "title-match";
    pub const KDL_ARTIST_MATCH: &str = "artist-match";
    pub const KDL_ALBUM_MATCH: &str = "album-match";
    pub const KDL_TRACK_NUMBER_MATCH: &str = "track-number-match";
}

/// Opinions for MusicBrainz release bin-packing.
///
/// Controls filtering thresholds for discarding poor-quality recording matches
/// before the greedy release assignment algorithm runs, plus MIS conflict
/// resolution parameters (knot extraction, tier ordering, discography reduction).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReleasePackingOpinions {
    /// Duration tolerance as a fraction (0.0-1.0). Recording matches where the
    /// duration differs by more than this fraction are discarded. Default: 0.15 (15%).
    pub duration_tolerance_pct: f64,
    /// Minimum AcoustID confidence to consider a recording match. Default: 0.3
    pub min_confidence: f64,
    /// Scoring weights for AcoustID-backed candidate assignment.
    pub candidate_weights: PackingWeights,
    /// Scoring weights for tag-only elimination/gap-filling assignment.
    pub elimination_weights: PackingWeights,
    /// Title similarity threshold for pre-assignment (elimination phase 1).
    /// File-slot pairs with similarity above this and exactly one candidate each
    /// are locked in before Hungarian runs. Default: 0.95.
    pub title_preassign_threshold: f64,
    /// Knot extraction: proposals/inodes ratio threshold (default 3.0, 0 to disable).
    /// Components where proposals/inodes >= this are extracted from MIS.
    pub packing_knot_ratio: f64,
    /// Knot extraction: component size limit (default 50, 0 to disable).
    /// Components larger than this are extracted regardless of ratio.
    pub packing_knot_size_limit: usize,
    /// Run Singles MIS round before Incompletes (default: true).
    /// When true, single-track releases claim inodes first, preventing
    /// single-file incomplete packings from entering the expensive MIS round.
    pub singles_before_incompletes: bool,
    /// When true, knots containing proposals that cover ALL contested inodes
    /// are reduced to only those covering proposals before greedy resolution.
    /// A lazy alternative to manual knot review for discography releases. Default: true.
    pub allow_resolve_knots_with_discographies: bool,
}

impl Default for ReleasePackingOpinions {
    fn default() -> Self {
        Self {
            duration_tolerance_pct: 0.15,
            min_confidence: 0.3,
            candidate_weights: PackingWeights::candidate_defaults(),
            elimination_weights: PackingWeights::elimination_defaults(),
            title_preassign_threshold: Self::DEFAULT_TITLE_PREASSIGN_THRESHOLD,
            packing_knot_ratio: Self::DEFAULT_PACKING_KNOT_RATIO,
            packing_knot_size_limit: Self::DEFAULT_PACKING_KNOT_SIZE_LIMIT,
            singles_before_incompletes: true,
            allow_resolve_knots_with_discographies: true,
        }
    }
}

impl ReleasePackingOpinions {
    pub const DEFAULT_TITLE_PREASSIGN_THRESHOLD: f64 = 0.95;
    pub const DEFAULT_PACKING_KNOT_RATIO: f64 = 3.0;
    pub const DEFAULT_PACKING_KNOT_SIZE_LIMIT: usize = 50;

    pub const KDL_DURATION_TOLERANCE_PCT: &str = "duration-tolerance-pct";
    pub const KDL_MIN_CONFIDENCE: &str = "min-confidence";
    pub const KDL_CANDIDATE_WEIGHTS: &str = "candidate-weights";
    pub const KDL_ELIMINATION_WEIGHTS: &str = "elimination-weights";
    pub const KDL_TITLE_PREASSIGN_THRESHOLD: &str = "title-preassign-threshold";
    pub const KDL_PACKING_KNOT_RATIO: &str = "packing-knot-ratio";
    pub const KDL_PACKING_KNOT_SIZE_LIMIT: &str = "packing-knot-size-limit";
    pub const KDL_SINGLES_BEFORE_INCOMPLETES: &str = "singles-before-incompletes";
    pub const KDL_ALLOW_DISCOGRAPHY_REDUCTION: &str = "allow-resolve-knots-with-discographies";
}

/// Directory granularity for inbox organize workflow.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum InboxOrganizeGranularity {
    /// Walk to deepest directories containing audio files (default).
    #[default]
    Leaf,
    /// Iterate only direct children of inbox/.
    TopLevel,
}

/// Opinions for inbox organize workflow.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct InboxOrganizeOpinions {
    /// How to group inbox directories for the organize workflow.
    pub directory_granularity: InboxOrganizeGranularity,
}

impl InboxOrganizeOpinions {
    pub const KDL_DIR_GRANULARITY: &str = "directory-granularity";
}

/// Configuration for external metadata matching (AcoustID, MusicBrainz).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExternalMatchingConfig {
    /// AcoustID API key. Empty string = disabled.
    pub acoustid_api_key: String,
    /// Rate limit: requests per second for AcoustID (default 3).
    pub requests_per_second: u32,
    /// Rate limit: requests per second for MusicBrainz (default 25).
    pub mb_requests_per_second: u32,
    /// MusicBrainz API base URL. Default: "https://musicbrainz.org/ws/2".
    /// Set to a local mirror (e.g. "https://mb.example.com/ws/2") to bypass rate limits.
    pub mb_base_url: String,
    /// Auto-trigger MB enrichment when AcoustID matches arrive (default true).
    pub auto_enrich_on_match: bool,
    /// How many days before re-fetching MB cache entries (default 30).
    pub mb_cache_ttl_days: u32,
    /// Locale preference order for artist names (BCP 47, e.g. ["en", "ja"]).
    /// Native script is always the final fallback.
    pub preferred_locales: Vec<String>,
    /// Tag templates: (UPPERCASE tag name, template string).
    /// Templates use `{var}` syntax for MB field substitution.
    pub tag_templates: Vec<(String, String)>,
}

/// Opinions for disc extraction from ALBUM and TRACKNUMBER tags.
///
/// Controls how extracted disc identifiers are written (tag name, letter mapping).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscExtractionOpinions {
    /// Tag name to write extracted disc identifier into. Default: "DISCNUMBER"
    pub disc_tag_name: String,
    /// Map letter prefixes to numbers (A→1, B→2, ...). Default: false
    pub map_letters_to_numbers: bool,
}

impl Default for DiscExtractionOpinions {
    fn default() -> Self {
        Self {
            disc_tag_name: "DISCNUMBER".to_string(),
            map_letters_to_numbers: false,
        }
    }
}

impl DiscExtractionOpinions {
    pub const KDL_DISC_TAG_NAME: &str = "disc-tag-name";
    pub const KDL_MAP_LETTERS: &str = "map-letters-to-numbers";
}

/// Controls whether sidecar images are deployed alongside audio files.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
pub enum SidecarDeployMode {
    /// Do not deploy sidecar images.
    Disabled,
    /// Deploy only the primary cover image (cover_front role) per album directory.
    #[default]
    PrimaryCover,
    /// Deploy all sidecar images found alongside audio files.
    All,
}

/// Album art embedding and upgrade configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AlbumArtOpinions {
    /// Whether to deploy sidecar cover images alongside audio files to libraries.
    /// Default: PrimaryCover (deploy only the primary cover image).
    pub sidecar_deploy_mode: SidecarDeployMode,
}

impl AlbumArtOpinions {
    pub const KDL_SIDECAR_DEPLOY: &str = "sidecar-deploy-mode";
}

/// Debug and diagnostic options.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugOpinions {
    /// Log periodic memory snapshots (RSS, SQLite, threads) to general.log.
    /// Default: false.
    pub memory_logging: bool,
}

impl DebugOpinions {
    pub const KDL_MEMORY_LOGGING: &str = "memory-logging";
}

impl Default for ExternalMatchingConfig {
    fn default() -> Self {
        Self {
            acoustid_api_key: String::new(),
            requests_per_second: 3,
            mb_requests_per_second: 25,
            mb_base_url: Self::DEFAULT_MB_BASE_URL.to_string(),
            auto_enrich_on_match: true,
            mb_cache_ttl_days: 30,
            preferred_locales: Vec::new(),
            tag_templates: Vec::new(),
        }
    }
}

impl ExternalMatchingConfig {
    pub const DEFAULT_MB_BASE_URL: &str = "https://musicbrainz.org/ws/2";

    pub const KDL_ACOUSTID_KEY: &str = "acoustid-api-key";
    pub const KDL_REQ_PER_SEC: &str = "requests-per-second";
    pub const KDL_MB_REQ_PER_SEC: &str = "mb-requests-per-second";
    pub const KDL_MB_BASE_URL: &str = "mb-base-url";
    pub const KDL_AUTO_ENRICH: &str = "auto-enrich-on-match";
    pub const KDL_MB_CACHE_TTL: &str = "mb-cache-ttl-days";
    pub const KDL_PREFERRED_LOCALES: &str = "preferred-locales";
    pub const KDL_TAG_TEMPLATES: &str = "tag-templates";
}

impl Default for Opinions {
    fn default() -> Self {
        Self {
            lossy_shit_formats_to_flac: false,
            quality_resolution: QualityResolutionOpinions::default(),
            canonicalization: CanonicalizationOpinions::default(),
            startup: StartupOpinions::default(),
            health_detection: HealthDetectionOpinions::default(),
            performance: PerformanceOpinions::default(),
            tag_splitting: TagSplittingOpinions::default(),
            duplicate_analysis: DuplicateAnalysisOpinions::default(),
            release_packing: ReleasePackingOpinions::default(),
            inbox_organize: InboxOrganizeOpinions::default(),
            idle_rescan_interval_secs: 180,
            leave_transactions_open: false,
            external_matching: ExternalMatchingConfig::default(),
            disc_extraction: DiscExtractionOpinions::default(),
            album_art: AlbumArtOpinions::default(),
            debug: DebugOpinions::default(),
        }
    }
}

/// A configured source directory within the corpus.
///
/// Source directories are the logical "collections" that files belong to.
/// They define where files deploy to and how duplicates between sources
/// should be resolved.
///
/// Boolean fields are `Option<bool>` — `None` means "inherit from parent
/// source directory" (or fall back to system default if no parent sets it).
/// This enables child dirs to override only specific settings without
/// shadowing the parent's other explicit values.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourceDir {
    /// Path relative to corpus root (e.g., "web/releases/bandcamp")
    pub path: PathBuf,
    /// Target library names for deployment (e.g., ["music", "soundtracks"])
    /// A single source can deploy to multiple libraries.
    /// Empty vec = inherit from parent source directory.
    pub libraries: Vec<String>,
    /// Whether duplicates from this source can be stashed when another source wins.
    /// None = inherit from parent (system default: true).
    pub can_stash_dupes: Option<bool>,
    /// Whether intra-source duplicates should be flagged.
    /// None = inherit from parent (system default: true).
    /// When resolved to false, duplicate groups entirely within this source are suppressed.
    pub interior_dupes: Option<bool>,
    /// Optional path-tag schema: expected file path structure expressed as tag placeholders.
    /// When set, files under this source dir are checked for path-tag agreement.
    /// None = inherit from parent.
    pub path_schema: Option<PathTagSchema>,
    /// Whether AcoustID lookups are enabled for this source directory.
    /// None = inherit from parent (system default: true).
    pub enable_acoustid: Option<bool>,
}

impl SourceDir {
    /// Whether this source dir has all-default/inherit settings and carries no information.
    ///
    /// A default entry (no libraries, all options None, no schema) is semantically
    /// empty — it configures nothing beyond what the absence of config already implies.
    /// Such entries are elided from dirs.kdl on write.
    pub fn is_default(&self) -> bool {
        self.libraries.is_empty()
            && self.can_stash_dupes.is_none()
            && self.interior_dupes.is_none()
            && self.path_schema.is_none()
            && self.enable_acoustid.is_none()
    }
}

/// Fully resolved source configuration for a specific path.
///
/// All fields have concrete values — inheritance has been flattened by walking
/// from most-specific to least-specific matching SourceDir and taking the first
/// explicitly-set value for each field. System defaults apply when nothing in
/// the chain sets a field.
#[derive(Debug, Clone)]
pub struct ResolvedSourceConfig {
    /// The most specific matching SourceDir's path (for prefix stripping).
    pub source_path: PathBuf,
    /// Resolved library names (first non-empty in chain, or empty).
    pub libraries: Vec<String>,
    /// Resolved can_stash_dupes (first Some in chain, or true).
    pub can_stash_dupes: bool,
    /// Resolved interior_dupes (first Some in chain, or true).
    pub interior_dupes: bool,
    /// Resolved path schema (first Some in chain, or None).
    pub path_schema: Option<PathTagSchema>,
}

/// Shared config wrapped in `Arc<RwLock<Config>>` for thread-safe read/write access.
///
/// The write lock is only held briefly during config updates (in Witch::tick()),
/// and since tick() and render() are sequential on the main thread, there is no
/// contention for UI reads.
pub type SharedConfig = Arc<RwLock<Config>>;

/// Convenience accessor for reading shared config without manual lock management.
pub fn read_shared_config(shared: &SharedConfig) -> RwLockReadGuard<'_, Config> {
    shared.read().expect("SharedConfig lock poisoned")
}

impl Config {
    /// Wrap this config in a shared Arc<RwLock> for centralized access.
    pub fn into_shared(self) -> SharedConfig {
        Arc::new(RwLock::new(self))
    }

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

    pub fn inbox_dir(&self) -> PathBuf {
        self.root.join("inbox")
    }

    // =========================================================================
    // Source directory queries
    // =========================================================================

    /// Get all corpus paths that deploy to a specific library.
    ///
    /// Returns absolute paths to corpus directories that are configured
    /// to deploy to the given library name.
    pub fn get_corpus_paths_for_library(&self, library_name: &str) -> Vec<PathBuf> {
        let corpus_dir = self.corpus_dir();
        self.source_dirs
            .iter()
            .filter(|sd| sd.libraries.contains(&library_name.to_string()))
            .map(|sd| corpus_dir.join(&sd.path))
            .collect()
    }

    /// Check if a file path is under a configured source directory.
    ///
    /// Returns true if the path starts with any configured source directory.
    /// Paths are expected in relative format: `corpus/<relative-path>`.
    pub fn is_path_in_source(&self, path: &Path) -> bool {
        self.source_dirs.iter().any(|sd| {
            let prefix = Path::new("corpus").join(&sd.path);
            path.starts_with(&prefix)
        })
    }

    /// Resolve the full source config for a corpus-relative path, with inheritance.
    ///
    /// Takes a path relative to corpus root (WITHOUT "corpus/" prefix),
    /// e.g., `web/releases/bandcamp/Artist/Album/track.flac`.
    /// Returns None if not under any configured source directory.
    ///
    /// For each field, walks from most-specific to least-specific source dir
    /// and returns the first explicitly-set value. Falls back to system defaults
    /// (true for bools, None for schema, empty for libraries) if nothing in the
    /// chain sets the field.
    ///
    /// `source_path` is always the most-specific matching SourceDir's path,
    /// used for prefix stripping and source identity.
    pub fn resolve_source_config(&self, relative_path: &Path) -> Option<ResolvedSourceConfig> {
        // Collect all matching sources, sorted most specific (longest path) first.
        let mut matching: Vec<&SourceDir> = self
            .source_dirs
            .iter()
            .filter(|sd| relative_path.starts_with(&sd.path))
            .collect();

        if matching.is_empty() {
            return None;
        }

        matching.sort_by(|a, b| b.path.as_os_str().len().cmp(&a.path.as_os_str().len()));

        let source_path = matching[0].path.clone();

        // Walk chain for each field: first explicit value wins, else system default.
        let libraries = matching
            .iter()
            .find(|sd| !sd.libraries.is_empty())
            .map(|sd| sd.libraries.clone())
            .unwrap_or_default();

        let can_stash_dupes = matching
            .iter()
            .find_map(|sd| sd.can_stash_dupes)
            .unwrap_or(true);

        let interior_dupes = matching
            .iter()
            .find_map(|sd| sd.interior_dupes)
            .unwrap_or(true);

        let path_schema = matching.iter().find_map(|sd| sd.path_schema.clone());

        Some(ResolvedSourceConfig {
            source_path,
            libraries,
            can_stash_dupes,
            interior_dupes,
            path_schema,
        })
    }

    /// Resolve source config for a DB path (with "corpus/" prefix).
    ///
    /// Strips the "corpus/" prefix and delegates to `resolve_source_config`.
    pub fn resolve_source_config_for_db_path(&self, db_path: &str) -> Option<ResolvedSourceConfig> {
        let relative = db_path.strip_prefix("corpus/").unwrap_or(db_path);
        self.resolve_source_config(Path::new(relative))
    }

    /// Get the raw SourceDir for an exact path match (for config editing).
    ///
    /// Unlike `resolve_source_config`, this returns the raw SourceDir with
    /// `Option<bool>` fields intact — used by the dir config editor to show
    /// what THIS directory explicitly sets vs. inherits.
    pub fn get_raw_source_dir(&self, relative_path: &Path) -> Option<&SourceDir> {
        self.source_dirs.iter().find(|sd| sd.path == relative_path)
    }

    // =========================================================================
    // Validation
    // =========================================================================

    /// Validate that the archive root exists and all subdirectories are on the same filesystem.
    ///
    /// This check ensures:
    /// - Hard links will work (corpus -> libraries)
    /// - Atomic moves will work (corpus -> stash)
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
                    first_name,
                    first_dir,
                    first_dev,
                    name,
                    dir,
                    dev
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

    /// Validate configuration.
    pub fn validate(&self) -> Result<()> {
        self.validate_same_filesystem()
            .context("Filesystem validation failed")?;

        // Validate source paths don't escape corpus
        let corpus_dir = self.corpus_dir();
        for source in &self.source_dirs {
            let full_path = corpus_dir.join(&source.path);
            if !full_path.starts_with(&corpus_dir) {
                anyhow::bail!("Source path escapes corpus directory: {:?}", source.path);
            }
        }

        Ok(())
    }
}
