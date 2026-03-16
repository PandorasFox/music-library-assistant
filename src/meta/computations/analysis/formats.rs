//! Format detection executors.
//!
//! Lossless remux candidate detection (non-Vorbis lossless containers).


use crate::db::types::Zone;
use crate::logging::log_general;
use crate::meta::computations::helpers::drop_stale_corpus_signal;
use crate::meta::computations::traits::ComputationContext;
use crate::meta::signals::data::LosslessRemuxSignal;
use crate::meta::signals::registry::TypedSignalWrite;

use super::{Computation, Result};

// ============================================================================
// Lossless Remux Candidate Detection
// ============================================================================

/// Lossless file types that should be remuxed to FLAC.
const LOSSLESS_REMUX_TYPES: &[&str] = &[
    "wav", "aiff", "aif", "ape", "wv",
];

const LOSSLESS_REMUX_COMPUTATION: &str = "lossless_remux";

/// Execute DetectLosslessRemux - detect lossless files in non-Vorbis containers.
///
/// Uses dirty inode tracking: only checks recently (re)indexed inodes instead
/// of scanning the entire corpus. Format is immutable after indexing, so only
/// newly-indexed files need checking.
pub fn execute_detect_lossless_remux(
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DetectLosslessRemux;

    let sender = require_sender!(computation);

    let dirty_inodes = match read_only_db.get_dirty_inodes(LOSSLESS_REMUX_COMPUTATION) {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query dirty inodes: {}", e),
            );
        }
    };

    if dirty_inodes.is_empty() {
        log_general("[COMPUTE] DetectLosslessRemux: no dirty inodes, skipping");
        return Result::success(computation, Vec::new());
    }

    let mut emitted = 0;
    let mut cleared = 0;

    for inode in &dirty_inodes {
        match read_only_db.get_audio_file_by_inode(*inode, Zone::Corpus) {
            Ok(Some(audio_file)) => {
                let file_type_lower = audio_file.audio.file_type.to_lowercase();
                if LOSSLESS_REMUX_TYPES.contains(&file_type_lower.as_str()) {
                    sender.write_typed_signal(
                        TypedSignalWrite::LosslessRemux(LosslessRemuxSignal {
                            inode: *inode,
                            path: audio_file.path().to_string(),
                            file_type: audio_file.audio.file_type.clone(),
                        }),
                        witness,
                    );
                    emitted += 1;
                } else {
                    // Not a remux candidate — clear any stale signal
                    drop_stale_corpus_signal::<LosslessRemuxSignal>(
                        read_only_db,
                        &sender,
                        *inode,
                        witness,
                    );
                    cleared += 1;
                }
            }
            _ => {
                // File gone from corpus — clear any stale signal
                drop_stale_corpus_signal::<LosslessRemuxSignal>(
                    read_only_db,
                    &sender,
                    *inode,
                    witness,
                );
                cleared += 1;
            }
        }

        sender.clear_dirty_inode(*inode, LOSSLESS_REMUX_COMPUTATION, witness);
    }

    log_general(format!(
        "[COMPUTE] DetectLosslessRemux: {} dirty inodes, emitted={}, cleared={}",
        dirty_inodes.len(),
        emitted,
        cleared
    ));

    Result::success(computation, Vec::new())
}
