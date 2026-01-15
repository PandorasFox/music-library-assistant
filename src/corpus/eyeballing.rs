//! Eyeballing Module
//!
//! Corpus state observation driven by the Eye. The eyeballing system detects
//! out-of-band changes to the corpus by comparing filesystem state to the
//! database index.
//!
//! ## Overview
//!
//! - **Startup**: Eyeballing ALWAYS runs before the Eye opens
//! - **Runtime**: Lazy eyeballing triggered by Eye blink rolls (roll of 13)
//!
//! ## Computation Chain
//!
//! ```text
//! WalkCorpus ──► CompareInodes ──► VerifyMtime ──► VerifyTags
//!                    │                   │              │
//!                    ▼                   ▼              ▼
//!             MissingFromDisk     (spawn next)   OutOfBandTagChange
//!             MissingFromIndex
//! ```

use crate::corpus::computations::Computation;
use crate::daemon::TaskDaemon;
use std::path::PathBuf;

/// Queue eyeballing computations for the corpus.
///
/// This queues the initial WalkCorpus computation which will chain into
/// CompareInodes and then into VerifyMtime/VerifyTags as needed.
///
/// # Arguments
///
/// * `daemon` - The task daemon to queue computations on
/// * `corpus_root` - Path to the corpus directory
/// * `legacy_library` - Optional path to legacy library
/// * `paranoid` - If true, verify tags for ALL files (not just mtime mismatches)
pub fn queue_eyeballing(
    daemon: &mut TaskDaemon,
    corpus_root: &PathBuf,
    legacy_library: Option<&PathBuf>,
    paranoid: bool,
) {
    // Queue WalkCorpus for main corpus with paranoid flag
    daemon.queue_computation(Computation::WalkCorpus {
        root: corpus_root.clone(),
        source: "corpus".to_string(),
        paranoid,
    });

    // Queue WalkCorpus for legacy library if configured
    if let Some(legacy) = legacy_library {
        daemon.queue_computation(Computation::WalkCorpus {
            root: legacy.clone(),
            source: "legacy".to_string(),
            paranoid,
        });
    }
}
