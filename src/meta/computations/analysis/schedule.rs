//! Schedule Content Analysis executor.

use std::time::Instant;

use crate::db::ReadOnlyDb;
use crate::logging::log_general;
use crate::meta::computations::helpers::get_configured_library_names;
use crate::meta::recomputation::RecomputationScope;

use super::{Computation, Result};

/// Execute ScheduleContentAnalysis - spawns content detection computations
/// filtered by recomputation scope.
///
/// - `scope = None`: startup — spawn all computations.
/// - `scope = Some(s)`: post-mutation — spawn only computations whose domains
///   overlap with `s`.
pub fn execute_schedule_content_analysis(
    read_only_db: &ReadOnlyDb<'_>,
    start: Instant,
    scope: &Option<RecomputationScope>,
) -> Result {
    let run_all = scope.is_none();
    let s = scope.unwrap_or(RecomputationScope::EMPTY);

    log_general(format!(
        "[COMPUTE] ScheduleContentAnalysis: scope={:?} (run_all={})",
        scope, run_all
    ));

    // Note: OOB tag change classification is now handled in Observation phase by VerifyTags
    //
    // AnalyzeFingerprintOverlaps and DetectCrossSourceOverlaps are NOT spawned here.
    // They depend on FingerprintOverlap signals written by DetectFingerprintOverlaps,
    // so DetectFingerprintOverlaps defers them behind a pipeline barrier.

    let mut spawn = Vec::new();

    // Tag-sensitive computations
    if run_all || s.contains(RecomputationScope::TAGS) {
        spawn.extend([
            Computation::DetectMissingTags,
            Computation::DetectTagCanonicalizations,
            Computation::DetectInconsistentAlbumArtist,
            Computation::DetectCompoundTagValues,
            Computation::DetectDiscExtractions,
        ]);
    }

    // File/identity-sensitive computations
    if run_all || s.contains(RecomputationScope::FILES) {
        spawn.extend([
            Computation::DetectFingerprintOverlaps,
            Computation::DetectDuplicateInodes,
            Computation::DetectShitFormats,
            Computation::IndexImageFile,
        ]);
    }

    // Cross-domain: tags OR files
    if run_all || s.touches_any(&[RecomputationScope::TAGS, RecomputationScope::FILES]) {
        spawn.push(Computation::DetectMetadataDuplicates);
        spawn.push(Computation::DetectPathTagMismatches);
    }

    // Deploy-sensitive: files OR deploy OR tags (deploy path depends on tags+files)
    if run_all
        || s.touches_any(&[
            RecomputationScope::FILES,
            RecomputationScope::DEPLOY,
            RecomputationScope::TAGS,
        ])
    {
        spawn.push(Computation::DetectDeployConflicts);
        // DetectReleaseOverlaps spawns DeriveCorpusDeployStatus after
        // wait_for_queue_drain(), ensuring overlap signals are visible.
        spawn.push(Computation::DetectReleaseOverlaps);
    }

    // Deploy health: tags OR deploy
    if run_all || s.touches_any(&[RecomputationScope::TAGS, RecomputationScope::DEPLOY]) {
        // Spawn DeriveDeployHealthSignals for each configured library
        if let Ok(config) = crate::config::load_config() {
            let library_names = get_configured_library_names(&config);
            log_general(format!(
                "[COMPUTE] ScheduleContentAnalysis: libraries_dir={:?}, configured_libraries={:?}",
                config.libraries_dir(),
                library_names,
            ));
            for library_name in library_names {
                let library_root = config.libraries_dir().join(&library_name);
                let corpus_path_prefixes = config.get_corpus_paths_for_library(&library_name);
                log_general(format!(
                    "[COMPUTE] ScheduleContentAnalysis: spawning DeriveDeployHealthSignals for '{}' root={:?} prefixes={:?}",
                    library_name, library_root, corpus_path_prefixes,
                ));
                spawn.push(Computation::DeriveDeployHealthSignals {
                    library_name,
                    library_root,
                    corpus_path_prefixes,
                });
            }
        } else {
            log_general("[COMPUTE] ScheduleContentAnalysis: WARNING - failed to load config, no library health computations spawned");
        }
    }

    // External match derivation: EXTERNAL (new data), TAGS (comparison baseline), FILES (new fingerprints)
    if run_all
        || s.touches_any(&[
            RecomputationScope::EXTERNAL,
            RecomputationScope::TAGS,
            RecomputationScope::FILES,
        ])
    {
        spawn.push(Computation::DeriveExternalMatches);
    }

    // Inbox computations
    if run_all || s.touches_any(&[RecomputationScope::INBOX, RecomputationScope::FILES]) {
        spawn.push(Computation::DetectInboxCorpusMatches);
    }
    if run_all || s.touches_any(&[RecomputationScope::INBOX, RecomputationScope::TAGS]) {
        spawn.push(Computation::DetectInboxTagCanonicity);
        spawn.push(Computation::DetectInboxMissingTags);
        spawn.push(Computation::DetectInboxCompoundTags);
    }

    let total_possible = 17; // approximate total without library-specific ones
    log_general(format!(
        "[COMPUTE] ScheduleContentAnalysis: spawning {} computations (of ~{} possible)",
        spawn.len(),
        total_possible
    ));

    // Suppress unused read_only_db warning - not used in this function
    let _ = read_only_db;

    Result::success(
        Computation::ScheduleContentAnalysis { scope: *scope },
        start.elapsed().as_millis() as u64,
        spawn,
    )
}
