//! Embeddable album art detection.
//!
//! Finds directories where a sidecar image file exists alongside audio files
//! that lack embedded album art pictures. Picture presence is recorded in
//! `audio_info.has_pictures` at index time, so this computation only does
//! cheap DB queries + filesystem sidecar image scanning (no lofty probing).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::corpus::paths;
use crate::logging::log_general;
use crate::meta::computations::helpers::{reconcile_aggregate_signals, ComputedAggregateSignal};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    EmbeddableAlbumArtData, EmbeddableAlbumArtSignal, TypedSignalWrite,
};
use crate::corpus::db::ReadOnlyDb;
use crate::db_thread;

use super::{Computation, Result};

/// Sidecar image filenames in priority order (case-insensitive matching).
const SIDECAR_PRIORITY_NAMES: &[&str] = &[
    "cover", "folder", "albumart", "album", "front",
];

/// Image file extensions we recognise.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// Find the best sidecar image file in a directory.
///
/// Priority: known names (cover > folder > albumart > album > front) then any image file.
/// Returns the absolute path to the image if found.
fn find_sidecar_image(dir: &Path) -> Option<PathBuf> {
    let entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return None,
    };

    // Build list of image files in this directory
    let mut image_files: Vec<PathBuf> = Vec::new();
    for entry in &entries {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|s| s.to_lowercase())
            .unwrap_or_default();
        if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
            image_files.push(path);
        }
    }

    if image_files.is_empty() {
        return None;
    }

    // Check priority names first
    for priority_name in SIDECAR_PRIORITY_NAMES {
        for img in &image_files {
            let stem = img
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if stem == *priority_name {
                return Some(img.clone());
            }
        }
    }

    // Fall back to any image file (first alphabetically for determinism)
    image_files.sort();
    image_files.into_iter().next()
}

/// Execute DetectEmbeddableAlbumArt — find directories with sidecar art and artless files.
///
/// Reads artless files from `audio_info.has_pictures = 0` (set at index time),
/// groups by parent directory, then scans filesystem for sidecar images.
/// No lofty probing occurs here.
pub fn execute_detect_embeddable_album_art(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectEmbeddableAlbumArt;

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

    let resolver = paths::get_resolver();

    // Load artless corpus files from DB (has_pictures = 0, healthy, corpus source)
    let artless_files = match read_only_db.get_artless_corpus_files() {
        Ok(f) => f,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to query artless files: {}", e),
            );
        }
    };

    // Group by parent directory (resolve to absolute path via corpus resolver)
    let mut by_directory: HashMap<PathBuf, Vec<(i64, String)>> = HashMap::new();
    for (inode, path_str) in &artless_files {
        let abs_path = resolver.resolve(Path::new(path_str));
        if let Some(parent) = abs_path.parent() {
            by_directory
                .entry(parent.to_path_buf())
                .or_default()
                .push((*inode, path_str.clone()));
        }
    }

    let mut computed = Vec::new();
    let mut dirs_with_sidecar = 0;
    let mut total_emitted = 0;

    for (dir, files) in &by_directory {
        // Check if directory has a sidecar image (filesystem scan, cheap)
        let sidecar = match find_sidecar_image(dir) {
            Some(s) => s,
            None => continue,
        };
        dirs_with_sidecar += 1;

        let image_filename = sidecar
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
            .to_string();

        // Files are already artless (from DB query) — no need to probe
        let mut artless_inodes = Vec::new();
        let mut artless_paths = Vec::new();

        for (inode, abs_path_str) in files {
            let rel = resolver
                .to_relative(Path::new(abs_path_str))
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|| abs_path_str.clone());
            artless_inodes.push(*inode);
            artless_paths.push(rel);
        }

        total_emitted += artless_inodes.len();

        // Key: relative directory path
        let key = resolver
            .to_relative(dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.to_string_lossy().to_string());

        computed.push(ComputedAggregateSignal {
            key: key.clone(),
            typed_data: TypedSignalWrite::EmbeddableAlbumArt(EmbeddableAlbumArtSignal {
                key,
                data: EmbeddableAlbumArtData {
                    image_path: sidecar.to_string_lossy().to_string(),
                    image_filename,
                    artless_inodes,
                    artless_paths,
                },
            }),
        });
    }

    let (cleared, written, _, _) = reconcile_aggregate_signals::<EmbeddableAlbumArtSignal>(
        read_only_db,
        &sender,
        computed,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DetectEmbeddableAlbumArt: {} artless files across {} dirs, \
         {} dirs had sidecar images, {} signals ({} artless files), cleared {} stale",
        artless_files.len(),
        by_directory.len(),
        dirs_with_sidecar,
        written,
        total_emitted,
        cleared,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
