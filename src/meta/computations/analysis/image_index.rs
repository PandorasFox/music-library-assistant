//! Image file indexing computation.
//!
//! Extracts image metadata (format, dimensions, role) from corpus image files
//! and writes it to the `image_info` table. Uses dirty-inode tracking: the
//! scanner marks image inodes dirty for `index_image_file`, and this computation
//! processes them.

use std::path::Path;
use std::time::Instant;

use crate::corpus::paths;
use crate::corpus::tags;
use crate::db::ReadOnlyDb;
use crate::db::write_thread;
use crate::logging::log_general;
use crate::meta::computations::analysis::album_art::{COVER_FRONT_NAMES, COVER_BACK_NAMES};
use crate::meta::computations::types::ComputationWitness;

use super::{Computation, Result};

/// Computation type identifier for dirty inode tracking.
const INDEX_IMAGE_FILE_COMPUTATION: &str = "index_image_file";

/// Execute IndexImageFile — extract image metadata for dirty inodes.
///
/// Iterates dirty inodes, reads image dimensions via lofty, determines role
/// from filename, and writes to `image_info`. Clears dirty flag after each.
pub fn execute_index_image_file(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::IndexImageFile;

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

    let dirty_inodes = match read_only_db.get_dirty_inodes(INDEX_IMAGE_FILE_COMPUTATION) {
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
        return Result::success(
            computation,
            start.elapsed().as_millis() as u64,
            Vec::new(),
        );
    }

    let resolver = paths::get_resolver();
    let mut indexed = 0;
    let mut skipped = 0;

    for inode in &dirty_inodes {
        match read_only_db.get_corpus_path_for_inode(*inode) {
            Ok(Some(rel_path)) => {
                let abs_path = resolver.resolve(Path::new(&rel_path));

                if !abs_path.exists() {
                    skipped += 1;
                    sender.clear_dirty_inode(*inode, INDEX_IMAGE_FILE_COMPUTATION, witness);
                    continue;
                }

                // Determine format from extension
                let ext = abs_path.extension()
                    .and_then(|e| e.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();
                let format = match ext.as_str() {
                    "jpg" | "jpeg" => "jpeg",
                    "png" => "png",
                    "gif" => "gif",
                    "bmp" => "bmp",
                    "webp" => "webp",
                    _ => "unknown",
                };

                // Determine role from filename stem
                let stem = abs_path.file_stem()
                    .and_then(|s| s.to_str())
                    .map(|s| s.to_lowercase())
                    .unwrap_or_default();

                let role = if COVER_FRONT_NAMES.iter().any(|&name| stem == name) {
                    "cover_front"
                } else if COVER_BACK_NAMES.iter().any(|&name| stem == name) {
                    "cover_back"
                } else {
                    "other"
                };

                // Read image dimensions
                let (width, height, _) = tags::image_dimensions(&abs_path);

                sender.upsert_image_info(*inode, format, width, height, role, witness);
                indexed += 1;
            }
            _ => {
                skipped += 1;
            }
        }

        sender.clear_dirty_inode(*inode, INDEX_IMAGE_FILE_COMPUTATION, witness);
    }

    log_general(format!(
        "[COMPUTE] IndexImageFile: {} dirty inodes, indexed={}, skipped={}",
        dirty_inodes.len(), indexed, skipped
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
