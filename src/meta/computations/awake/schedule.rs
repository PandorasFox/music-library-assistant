//! Schedule Content Analysis executor.

use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::helpers::get_configured_library_names;
use crate::corpus::db::ReadOnlyDb;

use super::{Computation, Result};

/// Execute ScheduleContentAnalysis - spawns all content detection computations.
pub fn execute_schedule_content_analysis(
    read_only_db: &ReadOnlyDb<'_>,
    start: Instant,
) -> Result {
    log_general("[COMPUTE] ScheduleContentAnalysis: spawning all detection computations");

    // Note: OOB tag change classification is now handled in Asleep phase by VerifyTags
    //
    // AnalyzeFingerprintOverlaps and ClusterDirectoryOverlaps are NOT spawned here.
    // They depend on FingerprintOverlap signals written by DetectFingerprintOverlaps,
    // so DetectFingerprintOverlaps spawns them after calling wait_for_queue_drain().
    let mut spawn = vec![
        Computation::DetectFingerprintOverlaps,
        Computation::DetectDuplicateInodes,
        Computation::DetectMissingTags,
        Computation::DetectMetadataDuplicates,
        Computation::DetectTagCanonicalizations,
        Computation::DetectInconsistentAlbumArtist,
        Computation::DetectCompoundTagValues,
        Computation::DetectShitFormats,
        Computation::DetectDeployConflicts,
        Computation::DeriveCorpusDeployStatus,
    ];

    // Also spawn DeriveDeployHealthSignals for each configured library
    // The scan data was stored in files table (source='library') during Awakening phase
    if let Ok(config) = crate::config::load_config() {
        let library_names = get_configured_library_names(&config);
        for library_name in library_names {
            let library_root = config.libraries_dir().join(&library_name);
            let corpus_path_prefixes = config.get_corpus_paths_for_library(&library_name);
            spawn.push(Computation::DeriveDeployHealthSignals {
                library_name,
                library_root,
                corpus_path_prefixes,
            });
        }
    }

    // Suppress unused read_only_db warning - not used in this function
    let _ = read_only_db;

    Result::success(
        Computation::ScheduleContentAnalysis,
        start.elapsed().as_millis() as u64,
        spawn,
    )
}
