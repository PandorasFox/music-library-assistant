//! Image file indexing computation.
//!
//! Indexes watcher-observed image files: extracts format, dimensions, and role
//! from file content, then writes to both `files` and `image_info` tables.

use crate::corpus::paths;
use crate::corpus::tags;
use crate::logging::log_general;
use crate::meta::computations::types::ComputationWitness;
use crate::witch::fs_watcher::ObservedImage;

pub(crate) use mm_meta::tags::{COVER_BACK_NAMES, COVER_FRONT_NAMES};

use super::{Computation, Result};

/// Execute IndexObservedImages — index watcher-observed image files.
///
/// The watcher provides FS-level data (inode, path, mtime, size). This
/// executor extracts format, dimensions, and role from the file content,
/// then writes to both `files` and `image_info` tables.
pub fn execute_index_observed_images(
    images: &[ObservedImage],
    witness: &ComputationWitness,
) -> Result {
    let computation = Computation::IndexObservedImages { images: images.to_vec() };
    let sender = require_sender!(computation);

    let resolver = paths::get_resolver();
    let mut indexed = 0;
    let mut skipped = 0;

    for img in images {
        let zone_str = match img.zone {
            crate::db::types::Zone::Corpus => "corpus",
            crate::db::types::Zone::Inbox => "inbox",
            crate::db::types::Zone::Library => "library",
        };

        // Register in files table (always — keeps path/mtime fresh)
        sender.index_image_file(
            &img.path,
            zone_str,
            img.inode,
            img.mtime_secs,
            img.mtime_nanos,
            img.file_size,
            witness,
        );

        // Extract format, dimensions, role from the actual file
        let abs_path = resolver.resolve(std::path::Path::new(&img.path));
        if !abs_path.exists() {
            skipped += 1;
            continue;
        }

        let ext = abs_path
            .extension()
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

        let stem = abs_path
            .file_stem()
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

        let (width, height, _) = tags::image_dimensions(&abs_path);

        sender.upsert_image_info(img.inode, format, width, height, role, witness);
        indexed += 1;
    }

    log_general(format!(
        "[COMPUTE] IndexObservedImages: {} images (indexed={}, skipped={})",
        images.len(), indexed, skipped,
    ));

    Result::success(computation, Vec::new())
}
