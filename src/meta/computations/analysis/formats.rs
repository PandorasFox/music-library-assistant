//! Format detection executors.
//!
//! Shit format detection (non-Vorbis containers).


use crate::db::types::Zone;
use crate::logging::log_general;
use crate::meta::computations::helpers::drop_stale_corpus_signal;
use crate::meta::computations::traits::ComputationContext;
use crate::meta::signals::data::{ShitFormatSignal};
use crate::meta::signals::registry::TypedSignalWrite;

use super::{Computation, Result};

// ============================================================================
// Shit Format Detection
// ============================================================================

/// File types that should trigger ShitFormat signal (non-Vorbis containers).
/// Includes lossy formats with poor metadata and lossless needing remux.
const SHIT_FORMAT_TYPES: &[&str] = &[
    "mp3", "m4a", "aac", "wma", "wav", "aiff", "aif", "ape", "wv",
];

const SHIT_FORMAT_COMPUTATION: &str = "shit_format";

/// Execute DetectShitFormats - detect files with non-Vorbis container formats.
///
/// Uses dirty inode tracking: only checks recently (re)indexed inodes instead
/// of scanning the entire corpus. Format is immutable after indexing, so only
/// newly-indexed files need checking.
pub fn execute_detect_shit_formats(
    ctx: &ComputationContext<'_>,
) -> Result {
    let read_only_db = ctx.read_db;
    let witness = ctx.witness;
    let computation = Computation::DetectShitFormats;

    let sender = require_sender!(computation);

    let dirty_inodes = match read_only_db.get_dirty_inodes(SHIT_FORMAT_COMPUTATION) {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                computation,
                format!("Failed to query dirty inodes: {}", e),
            );
        }
    };

    if dirty_inodes.is_empty() {
        log_general("[COMPUTE] DetectShitFormats: no dirty inodes, skipping");
        return Result::success(computation, Vec::new());
    }

    let mut emitted = 0;
    let mut cleared = 0;

    for inode in &dirty_inodes {
        match read_only_db.get_audio_file_by_inode(*inode, Zone::Corpus) {
            Ok(Some(audio_file)) => {
                let file_type_lower = audio_file.audio.file_type.to_lowercase();
                if SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str()) {
                    sender.write_typed_signal(
                        TypedSignalWrite::ShitFormat(ShitFormatSignal {
                            inode: *inode,
                            path: audio_file.path().to_string(),
                            file_type: audio_file.audio.file_type.clone(),
                        }),
                        witness,
                    );
                    emitted += 1;
                } else {
                    // Not a shit format — clear any stale signal
                    drop_stale_corpus_signal::<ShitFormatSignal>(
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
                drop_stale_corpus_signal::<ShitFormatSignal>(
                    read_only_db,
                    &sender,
                    *inode,
                    witness,
                );
                cleared += 1;
            }
        }

        sender.clear_dirty_inode(*inode, SHIT_FORMAT_COMPUTATION, witness);
    }

    log_general(format!(
        "[COMPUTE] DetectShitFormats: {} dirty inodes, emitted={}, cleared={}",
        dirty_inodes.len(),
        emitted,
        cleared
    ));

    Result::success(computation, Vec::new())
}
