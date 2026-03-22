//! Image file indexing and metadata analysis computations.
//!
//! Split into two phases:
//! - `IndexObservedImages` (fast): registers files in the `files` table so
//!   derivation doesn't see them as ghosts. No image decoding.
//! - `AnalyzeImageMetadata` (slow): opens each file to extract format,
//!   dimensions, and role. Writes to `image_info` table. Does not gate startup.

use crate::corpus::paths;
use crate::corpus::tags;
use crate::logging::log_general;
use crate::meta::computations::traits::ComputationContext;
use crate::witch::fs_thread::ObservedImage;

pub(crate) use mm_meta::tags::{COVER_BACK_NAMES, COVER_FRONT_NAMES};

use super::{Computation, Result};

/// Execute IndexObservedImages — fast file registration only.
///
/// Writes path/inode/mtime/size to the `files` table for each observed image.
/// Spawns `AnalyzeImageMetadata` as a follow-up for the slow decode pass.
pub fn execute_index_observed_images(
    ctx: &ComputationContext<'_>,
    images: &[ObservedImage],
) -> Result {
    let witness = ctx.witness;
    let computation = Computation::IndexObservedImages { images: images.to_vec() };
    let sender = require_sender!(computation);

    for img in images {
        let zone_str = match img.zone {
            crate::db::types::Zone::Corpus => "corpus",
            crate::db::types::Zone::Library => "library",
        };

        sender.index_image_file(
            &img.path,
            zone_str,
            img.inode,
            img.mtime_secs,
            img.mtime_nanos,
            img.file_size,
            witness,
        );
    }

    log_general(format!(
        "[COMPUTE] IndexObservedImages: registered {} images in files table",
        images.len(),
    ));

    // Spawn the slow metadata analysis as a follow-up — doesn't gate derivation.
    let spawn = vec![Computation::AnalyzeImageMetadata { images: images.to_vec() }];
    Result::success(computation, spawn)
}

/// Execute AnalyzeImageMetadata — slow image decode pass.
///
/// Opens each file to extract format, dimensions, and cover role, then
/// writes to `image_info`. This runs after derivation has already started.
pub fn execute_analyze_image_metadata(
    ctx: &ComputationContext<'_>,
    images: &[ObservedImage],
) -> Result {
    let witness = ctx.witness;
    let computation = Computation::AnalyzeImageMetadata { images: images.to_vec() };
    let sender = require_sender!(computation);

    let resolver = paths::get_resolver();
    let mut analyzed = 0;
    let mut skipped = 0;

    for img in images {
        let abs_path = resolver.resolve_for_zone(img.zone, std::path::Path::new(&img.path));
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
        analyzed += 1;
    }

    log_general(format!(
        "[COMPUTE] AnalyzeImageMetadata: {} images (analyzed={}, skipped={})",
        images.len(), analyzed, skipped,
    ));

    Result::success(computation, Vec::new())
}
