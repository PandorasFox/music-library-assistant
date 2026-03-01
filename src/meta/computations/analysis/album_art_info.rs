//! Album art info backfill computation.
//!
//! Extracts picture metadata (format, resolution, count) from existing corpus
//! files and writes it to the `audio_info` table. Uses dirty-inode tracking:
//! migration v24→v25 seeds all corpus inodes, and each inode is processed once.

use std::path::Path;
use std::time::Instant;

use crate::corpus::paths;
use crate::corpus::tags::TagSet;
use crate::db::ReadOnlyDb;
use crate::db::write_thread;
use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;

use super::{Computation, Result};

/// Computation type identifier for dirty inode tracking.
const ALBUM_ART_INFO_COMPUTATION: &str = "album_art_info";

/// Execute BackfillAlbumArtInfo - extract picture metadata for dirty inodes.
///
/// Single-pass computation: iterates dirty inodes, extracts picture metadata
/// via lofty, and writes pic_format/pic_width/pic_height/pic_count to audio_info.
/// Clears dirty flag after each inode regardless of outcome.
pub fn execute_backfill_album_art_info(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::BackfillAlbumArtInfo;

    let sender = match write_thread::signal_sender() {
        Some(s) => s.clone(),
        None => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                "DB thread not initialized".to_string(),
            );
        }
    };

    let dirty_inodes = match read_only_db.get_dirty_inodes(ALBUM_ART_INFO_COMPUTATION) {
        Ok(inodes) => inodes,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query dirty inodes: {}", e),
            );
        }
    };

    if dirty_inodes.is_empty() {
        log_general("[COMPUTE] BackfillAlbumArtInfo: no dirty inodes, skipping");
        // Still spawn DetectEmbeddableAlbumArt — embed detection for artless files
        // doesn't depend on backfill, only upgrade detection does.
        return Result::success(
            computation,
            start.elapsed().as_millis() as u64,
            vec![Computation::DetectEmbeddableAlbumArt],
        );
    }

    let resolver = paths::get_resolver();
    let mut updated = 0;
    let mut skipped = 0;

    for inode in &dirty_inodes {
        match read_only_db.get_corpus_path_for_inode(*inode) {
            Ok(Some(rel_path)) => {
                let abs_path = resolver.resolve(Path::new(&rel_path));
                let pic_info = TagSet::extract_picture_info(&abs_path);

                if let Some(info) = pic_info {
                    sender.update_picture_metadata(
                        *inode,
                        Some(info.format),
                        Some(info.width),
                        Some(info.height),
                        info.count,
                        witness,
                    );
                    updated += 1;
                } else {
                    // No pictures — write zeroed metadata to confirm we checked
                    sender.update_picture_metadata(
                        *inode,
                        None,
                        None,
                        None,
                        0,
                        witness,
                    );
                    skipped += 1;
                }
            }
            _ => {
                // Inode gone from corpus — just clear the dirty flag
                skipped += 1;
            }
        }

        sender.clear_dirty_inode(*inode, ALBUM_ART_INFO_COMPUTATION, witness);
    }

    log_general(format!(
        "[COMPUTE] BackfillAlbumArtInfo: {} dirty inodes, updated={}, skipped={}",
        dirty_inodes.len(), updated, skipped
    ));

    // Wait for picture metadata writes to land, then spawn album art detection
    // which depends on pic_format/pic_width/pic_height being populated.
    write_thread::wait_for_queue_drain();

    let follow_ups = if updated > 0 {
        vec![Computation::DetectEmbeddableAlbumArt]
    } else {
        Vec::new()
    };

    Result::success(computation, start.elapsed().as_millis() as u64, follow_ups)
}
