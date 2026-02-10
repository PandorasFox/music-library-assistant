//! Format detection executors.
//!
//! Shit format detection (non-Vorbis containers).

use std::time::Instant;

use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::corpus::db::types::FileSource;
use crate::meta::signals::data::{ShitFormatSignal, TypedSignalWrite};
use crate::corpus::db::ReadOnlyDb;
use crate::db_thread;

use super::{Computation, Result};

// ============================================================================
// Shit Format Detection
// ============================================================================

/// Execute DetectShitFormats - detect files with non-Vorbis container formats.
///
/// Queries all tracks and emits ShitFormat signals for those with file types
/// that have poor metadata support or inefficient containers (MP3, M4A, WAV, etc).
pub fn execute_detect_shit_formats(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectShitFormats;

    let sender = match db_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    /// File types that should trigger ShitFormat signal (non-Vorbis containers)
    /// Includes lossy formats with poor metadata and lossless needing remux
    const SHIT_FORMAT_TYPES: &[&str] = &["mp3", "m4a", "aac", "wma", "wav", "aiff", "aif", "ape", "wv"];

    // Clear all existing ShitFormat signals and rebuild
    sender.clear_all_of_corpus_type::<ShitFormatSignal>(witness);

    // Query all audio files and filter for shit formats
    let audio_files = match read_only_db.get_all_audio_files(FileSource::Corpus) {
        Ok(f) => f,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query audio files: {}", e),
            );
        }
    };

    let mut signal_count = 0;

    for audio_file in audio_files {
        let file_type_lower = audio_file.audio.file_type.to_lowercase();
        if SHIT_FORMAT_TYPES.contains(&file_type_lower.as_str()) {
            sender.write_typed_signal(
                TypedSignalWrite::ShitFormat(ShitFormatSignal {
                    inode: audio_file.inode(),
                    path: audio_file.path().to_string(),
                    file_type: audio_file.audio.file_type.clone(),
                }),
                witness,
            );

            signal_count += 1;
        }
    }

    log_general(format!(
        "[COMPUTE] DetectShitFormats: emitted {} ShitFormat signals",
        signal_count
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
