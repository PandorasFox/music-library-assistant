//! Config Edit Mutation
//!
//! Writes edited config to disk (comment-preserving KDL modification).
//! Returns the new Config in-band via TaskResult.config_update.

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::meta::computations::Computation;
use crate::meta::mutations::types::{DiffEntry, MutationResult, SignalClearScope, SignalToClear};
use crate::meta::recomputation::RecomputationScope;
use super::traits::{MutationContext, MutationExecutor};

/// Mutation that applies config edits to disk.
///
/// Carries the original KDL text (for comment-preserving modification),
/// the old config (for diffing), and the new config (to write).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplyConfigEditsMutation {
    /// The original KDL text from the config file.
    pub original_kdl: String,
    /// The config as it was before editing (for diffing).
    pub old_config: Config,
    /// The new config with edits applied.
    pub new_config: Config,
}

// Manual PartialEq — Config doesn't derive PartialEq, but Mutation enum requires it.
// Config edits are always unique (compare by original_kdl identity).
impl PartialEq for ApplyConfigEditsMutation {
    fn eq(&self, other: &Self) -> bool {
        self.original_kdl == other.original_kdl
    }
}

impl MutationExecutor for ApplyConfigEditsMutation {
    fn label(&self) -> &'static str {
        "Config update"
    }
    fn staging(&self) -> super::traits::MutationStaging { super::traits::MutationStaging::Staged(super::traits::MutationExecutionStage::Config) }

    fn execute(&self, _ctx: &MutationContext) -> MutationResult {
        match crate::config::write_config_to_disk(&self.original_kdl, &self.old_config, &self.new_config) {
            Ok(()) => {
                crate::logging::log_general("[CONFIG] Config written to disk successfully");
                MutationResult {
                    _mutation: super::Mutation::ApplyConfigEdits(self.clone()),
                    success: true,
                    error: None,
                    _duration_ms: 0, // Overwritten by caller
                    spawn_mutations: Vec::new(),
                    pending_signals: Vec::new(),
                    discovered_inodes: Vec::new(),
                }
            }
            Err(e) => {
                crate::logging::log_error(format!("[CONFIG] Config write failed: {:#}", e));
                MutationResult {
                    _mutation: super::Mutation::ApplyConfigEdits(self.clone()),
                    success: false,
                    error: Some(format!("Config write failed: {:#}", e)),
                    _duration_ms: 0,
                    spawn_mutations: Vec::new(),
                    pending_signals: Vec::new(),
                    discovered_inodes: Vec::new(),
                }
            }
        }
    }

    fn signal_clear_scope(&self) -> SignalClearScope {
        SignalClearScope::None
    }

    fn affected_inodes(&self) -> Vec<i64> {
        Vec::new()
    }

    fn additional_computations(&self) -> Vec<Computation> {
        let new_pairs = new_tag_separator_pairs(&self.old_config, &self.new_config);
        if new_pairs.is_empty() {
            return Vec::new();
        }
        vec![Computation::Analysis(
            crate::meta::computations::analysis::Computation::SeedCompoundTagDirtyInodes {
                new_separators: new_pairs,
            },
        )]
    }

    fn specific_signals_to_clear(&self) -> Vec<SignalToClear> {
        Vec::new()
    }

    fn paths_for_signal_updates(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    fn recomputation_scope(&self) -> RecomputationScope {
        config_recomputation_scope(&self.old_config, &self.new_config)
    }

    fn diff_entries(&self) -> Vec<DiffEntry> {
        config_diff_entries(&self.old_config, &self.new_config)
    }
}

/// Compare two Config structs field-by-field and produce diff entries for changed fields.
fn config_diff_entries(old: &Config, new: &Config) -> Vec<DiffEntry> {
    let mut diffs = Vec::new();
    let o = &old.opinions;
    let n = &new.opinions;

    macro_rules! cmp {
        ($label:expr, $old_val:expr, $new_val:expr) => {
            if $old_val != $new_val {
                diffs.push(DiffEntry::new($label, $old_val, $new_val));
            }
        };
        (float $label:expr, $old_val:expr, $new_val:expr) => {
            if ($old_val - $new_val).abs() >= f64::EPSILON {
                diffs.push(DiffEntry::new($label, $old_val, $new_val));
            }
        };
        (debug $label:expr, $old_val:expr, $new_val:expr) => {
            if $old_val != $new_val {
                diffs.push(DiffEntry::new($label, format!("{:?}", $old_val), format!("{:?}", $new_val)));
            }
        };
        (opt $label:expr, $old_val:expr, $new_val:expr) => {
            if $old_val != $new_val {
                let old_s = match $old_val { Some(v) => v.to_string(), None => "auto".to_string() };
                let new_s = match $new_val { Some(v) => v.to_string(), None => "auto".to_string() };
                diffs.push(DiffEntry::new($label, old_s, new_s));
            }
        };
        (list $label:expr, $old_val:expr, $new_val:expr) => {
            if $old_val != $new_val {
                diffs.push(DiffEntry::new($label, $old_val.join(", "), $new_val.join(", ")));
            }
        };
    }

    // General
    cmp!("Transcode lossy to FLAC", o.lossy_shit_formats_to_flac, n.lossy_shit_formats_to_flac);

    // Startup
    cmp!("Force full check at startup", o.startup.force_check_all_files_at_startup, n.startup.force_check_all_files_at_startup);
    cmp!(float "Vacuum threshold", o.startup.vacuum_threshold, n.startup.vacuum_threshold);
    cmp!(debug "Default view", o.startup.default_view, n.startup.default_view);

    // Quality Resolution
    cmp!(float "QR: Inbox bitrate fuzz %", o.quality_resolution.inbox_bitrate_fuzz_percent, n.quality_resolution.inbox_bitrate_fuzz_percent);

    // Canonicalization
    cmp!("Strip album format suffixes", o.canonicalization.strip_album_format_suffixes, n.canonicalization.strip_album_format_suffixes);

    // Health Detection
    cmp!(list "Required tags", o.health_detection.required_tags, n.health_detection.required_tags);
    cmp!("Album artist only if compilation", o.health_detection.album_artist_only_required_if_compilation, n.health_detection.album_artist_only_required_if_compilation);
    cmp!("Single album suffix", &o.health_detection.single_album_suffix, &n.health_detection.single_album_suffix);

    // Duplicate Analysis
    cmp!(float "Dup: FP similarity threshold", o.duplicate_analysis.fingerprint_similarity_threshold, n.duplicate_analysis.fingerprint_similarity_threshold);
    cmp!("Dup: Duration tolerance (ms)", o.duplicate_analysis.duration_tolerance_ms, n.duplicate_analysis.duration_tolerance_ms);
    cmp!("Dup: Elide variant titles", o.duplicate_analysis.elide_variant_titles, n.duplicate_analysis.elide_variant_titles);

    // Release Packing
    cmp!(float "Release packing: Duration tolerance %", o.release_packing.duration_tolerance_pct, n.release_packing.duration_tolerance_pct);
    cmp!(float "Release packing: Min confidence", o.release_packing.min_confidence, n.release_packing.min_confidence);

    // Inbox Organize
    cmp!(debug "Inbox organize granularity", o.inbox_organize.directory_granularity, n.inbox_organize.directory_granularity);

    // External Matching
    cmp!("AcoustID API key", &o.external_matching.acoustid_api_key, &n.external_matching.acoustid_api_key);
    cmp!("AcoustID requests/sec", o.external_matching.requests_per_second, n.external_matching.requests_per_second);

    // Performance
    cmp!(opt "Worker threads", o.performance.worker_threads, n.performance.worker_threads);
    cmp!("DB cache (MB)", o.performance.db_cache_mb, n.performance.db_cache_mb);
    cmp!("Timing instrumentation", o.performance.timing_instrumentation, n.performance.timing_instrumentation);

    diffs
}

/// Extract (tag_name, separator) pairs that are new in the updated config.
///
/// Compares old and new tag_splitting.tag_separators, returning pairs where
/// the separator was not present for that tag in the old config.
fn new_tag_separator_pairs(old: &Config, new: &Config) -> Vec<(String, String)> {
    let old_seps = &old.opinions.tag_splitting.tag_separators;
    let new_seps = &new.opinions.tag_splitting.tag_separators;
    let mut pairs = Vec::new();
    for (tag, new_sep_list) in new_seps {
        let old_sep_list = old_seps.get(tag);
        for sep in new_sep_list {
            let is_new = old_sep_list.map_or(true, |old| !old.contains(sep));
            if is_new {
                pairs.push((tag.clone(), sep.clone()));
            }
        }
    }
    pairs
}

/// Determine which domains a config change affects.
///
/// Inspects old vs new config field-by-field to produce a precise scope.
/// Fields that only affect runtime policy (startup, performance, quality resolution)
/// return EMPTY — they don't need content re-analysis.
fn config_recomputation_scope(old: &Config, new: &Config) -> RecomputationScope {
    let o = &old.opinions;
    let n = &new.opinions;
    let mut scope = RecomputationScope::EMPTY;

    // TAGS: fields that affect tag-sensitive computations
    if o.health_detection.required_tags != n.health_detection.required_tags
        || o.health_detection.album_artist_only_required_if_compilation != n.health_detection.album_artist_only_required_if_compilation
        || o.health_detection.single_album_suffix != n.health_detection.single_album_suffix
        || o.canonicalization.strip_album_format_suffixes != n.canonicalization.strip_album_format_suffixes
        || o.tag_splitting != n.tag_splitting
    {
        scope |= RecomputationScope::TAGS;
    }

    // FILES: fields that affect file/fingerprint/duplicate detection
    if o.duplicate_analysis != n.duplicate_analysis
        || o.lossy_shit_formats_to_flac != n.lossy_shit_formats_to_flac
    {
        scope |= RecomputationScope::FILES;
    }

    // INBOX: fields that affect inbox computations
    if o.inbox_organize != n.inbox_organize {
        scope |= RecomputationScope::INBOX;
    }

    // These fields are runtime policy — no content re-analysis needed:
    // startup.*, performance.*, idle_rescan_interval_secs, leave_transactions_open,
    // quality_resolution.*

    scope
}
