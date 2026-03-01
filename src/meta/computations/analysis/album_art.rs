//! Embeddable and upgradeable album art detection.
//!
//! Finds directories where sidecar image files exist alongside audio files
//! that either lack embedded album art (embeddable) or have lower-quality
//! embedded art than the sidecar (upgradeable).
//!
//! Picture presence is recorded in `audio_info.has_pictures` at index time,
//! and picture metadata (format, resolution) in `pic_format`/`pic_width`/
//! `pic_height` columns. This computation only does cheap DB queries +
//! filesystem sidecar image scanning (no lofty probing of audio files).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use crate::corpus::paths;
use crate::corpus::tags::{self, PictureInfo};
use crate::logging::log_general;
use crate::meta::computations::helpers::{reconcile_aggregate_signals, ComputedAggregateSignal};
use crate::meta::computations::types::ComputationWitness;
use crate::meta::signals::data::{
    EmbeddableAlbumArtData, EmbeddableAlbumArtSignal, PictureRole, SidecarImage,
    TypedSignalWrite, UpgradeableAlbumArtData, UpgradeableAlbumArtSignal,
};
use crate::db::ReadOnlyDb;
use crate::db::write_thread;

use super::{Computation, Result};

/// Sidecar image filenames that map to CoverFront, in priority order.
const COVER_FRONT_NAMES: &[&str] = &["cover", "folder", "albumart", "album", "front"];

/// Sidecar image filenames that map to CoverBack.
const COVER_BACK_NAMES: &[&str] = &["back"];

/// Image file extensions we recognise.
const IMAGE_EXTENSIONS: &[&str] = &["jpg", "jpeg", "png", "webp", "gif", "bmp"];

/// Find all sidecar image files in a directory, ordered by priority.
///
/// Returns images ordered: CoverFront names (cover > folder > albumart > album > front),
/// then CoverBack names, then remaining images sorted alphabetically.
fn find_sidecar_images(dir: &Path) -> Vec<(PathBuf, PictureRole)> {
    let entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).collect(),
        Err(_) => return Vec::new(),
    };

    // Collect all image files
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
        return Vec::new();
    }

    let mut result: Vec<(PathBuf, PictureRole)> = Vec::new();
    let mut used: Vec<bool> = vec![false; image_files.len()];

    // CoverFront priority names first
    for priority_name in COVER_FRONT_NAMES {
        for (i, img) in image_files.iter().enumerate() {
            if used[i] {
                continue;
            }
            let stem = img
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if stem == *priority_name {
                result.push((img.clone(), PictureRole::CoverFront));
                used[i] = true;
            }
        }
    }

    // CoverBack names
    for back_name in COVER_BACK_NAMES {
        for (i, img) in image_files.iter().enumerate() {
            if used[i] {
                continue;
            }
            let stem = img
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_lowercase())
                .unwrap_or_default();
            if stem == *back_name {
                result.push((img.clone(), PictureRole::CoverBack));
                used[i] = true;
            }
        }
    }

    // Remaining images sorted alphabetically, role = Other
    let mut remaining: Vec<_> = image_files
        .iter()
        .enumerate()
        .filter(|(i, _)| !used[*i])
        .map(|(_, img)| img.clone())
        .collect();
    remaining.sort();
    for img in remaining {
        result.push((img, PictureRole::Other));
    }

    result
}

/// Build SidecarImage metadata from a filesystem path and role.
///
/// Extracts image dimensions via lofty. This is a filesystem read but only
/// for image files (not audio files), so it's cheap.
fn build_sidecar_image(path: &Path, role: PictureRole) -> SidecarImage {
    let (width, height, format) = tags::image_dimensions(path);
    let filename = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    SidecarImage {
        path: path.to_string_lossy().to_string(),
        filename,
        role,
        format,
        width,
        height,
    }
}

/// Execute DetectEmbeddableAlbumArt — find directories with sidecar art and
/// artless files, and directories where sidecar art is better than embedded art.
///
/// Emits two signal types:
/// - `EmbeddableAlbumArt`: directories with artless files + sidecar images
/// - `UpgradeableAlbumArt`: directories where sidecar is better than embedded art
pub fn execute_detect_embeddable_album_art(
    read_only_db: &ReadOnlyDb<'_>,
    witness: &ComputationWitness,
    start: Instant,
) -> Result {
    let computation = Computation::DetectEmbeddableAlbumArt;

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

    let resolver = paths::get_resolver();

    let min_res = match crate::config::load_config() {
        Ok(c) => c.opinions.album_art.min_acceptable_resolution,
        Err(e) => {
            return Result::failure(
                computation,
                start.elapsed().as_millis() as u64,
                format!("Failed to load config: {}", e),
            );
        }
    };

    // Load artless corpus files from DB (has_pictures = 0, healthy, corpus)
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

    // Load corpus files with picture metadata (for upgrade detection)
    let files_with_art = match read_only_db.get_corpus_files_with_picture_info() {
        Ok(f) => f,
        Err(e) => {
            log_general(format!(
                "[COMPUTE] DetectEmbeddableAlbumArt: WARNING - failed to query files with picture info: {}, skipping upgrade detection",
                e
            ));
            Vec::new()
        }
    };

    // Group artless files by parent directory
    let mut artless_by_dir: HashMap<PathBuf, Vec<(i64, String)>> = HashMap::new();
    for (inode, path_str) in &artless_files {
        let abs_path = resolver.resolve(Path::new(path_str));
        if let Some(parent) = abs_path.parent() {
            artless_by_dir
                .entry(parent.to_path_buf())
                .or_default()
                .push((*inode, path_str.clone()));
        }
    }

    // Group files-with-art by parent directory
    let mut art_by_dir: HashMap<PathBuf, Vec<(i64, String, String, u32, u32)>> = HashMap::new();
    for (inode, path_str, pic_format, pic_width, pic_height) in &files_with_art {
        let abs_path = resolver.resolve(Path::new(path_str));
        if let Some(parent) = abs_path.parent() {
            art_by_dir
                .entry(parent.to_path_buf())
                .or_default()
                .push((*inode, path_str.clone(), pic_format.clone(), *pic_width, *pic_height));
        }
    }

    // Collect all unique directories that have either artless or art files
    let mut all_dirs: Vec<PathBuf> = artless_by_dir.keys().chain(art_by_dir.keys()).cloned().collect();
    all_dirs.sort();
    all_dirs.dedup();

    // Sidecar image cache: only scan each directory once
    let mut sidecar_cache: HashMap<PathBuf, Vec<SidecarImage>> = HashMap::new();

    let mut embeddable_computed = Vec::new();
    let mut upgradeable_computed = Vec::new();
    let mut dirs_with_sidecar = 0;
    let mut total_artless_emitted = 0;
    let mut total_upgradeable_emitted = 0;

    for dir in &all_dirs {
        // Find sidecar images in this directory
        let sidecars = sidecar_cache.entry(dir.clone()).or_insert_with(|| {
            find_sidecar_images(dir)
                .into_iter()
                .map(|(path, role)| build_sidecar_image(&path, role))
                .collect()
        });

        if sidecars.is_empty() {
            continue;
        }
        dirs_with_sidecar += 1;

        let primary_sidecar = &sidecars[0];

        let key = resolver
            .to_relative(dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|| dir.to_string_lossy().to_string());

        // Embeddable: artless files in this directory
        if let Some(files) = artless_by_dir.get(dir) {
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

            total_artless_emitted += artless_inodes.len();

            embeddable_computed.push(ComputedAggregateSignal::new(
                key.clone(),
                TypedSignalWrite::EmbeddableAlbumArt(EmbeddableAlbumArtSignal {
                    key: key.clone(),
                    data: EmbeddableAlbumArtData {
                        image_path: primary_sidecar.path.clone(),
                        image_filename: primary_sidecar.filename.clone(),
                        artless_inodes,
                        artless_paths,
                        sidecar_images: sidecars.clone(),
                    },
                }),
            ));
        }

        // Upgradeable: files with art where sidecar is better
        if let Some(files) = art_by_dir.get(dir) {
            let mut upgradeable_inodes = Vec::new();
            let mut upgradeable_paths = Vec::new();
            let mut representative_embedded: Option<(String, u32, u32)> = None;

            for (inode, abs_path_str, pic_format, pic_width, pic_height) in files {
                // Skip files whose embedded art already meets minimum resolution
                if min_res > 0 && *pic_width >= min_res && *pic_height >= min_res {
                    continue;
                }

                let embedded = PictureInfo {
                    format: pic_format.clone(),
                    width: *pic_width,
                    height: *pic_height,
                    count: 1,
                };

                if tags::is_sidecar_better(
                    &primary_sidecar.format,
                    primary_sidecar.width,
                    primary_sidecar.height,
                    &embedded,
                ) {
                    let rel = resolver
                        .to_relative(Path::new(abs_path_str))
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_else(|| abs_path_str.clone());
                    upgradeable_inodes.push(*inode);
                    upgradeable_paths.push(rel);
                    if representative_embedded.is_none() {
                        representative_embedded = Some((pic_format.clone(), *pic_width, *pic_height));
                    }
                }
            }

            if !upgradeable_inodes.is_empty() {
                total_upgradeable_emitted += upgradeable_inodes.len();
                let (emb_format, emb_width, emb_height) =
                    representative_embedded.unwrap_or(("unknown".to_string(), 0, 0));

                let upgrade_key = format!("upgrade:{}", key);
                upgradeable_computed.push(ComputedAggregateSignal::new(
                    upgrade_key.clone(),
                    TypedSignalWrite::UpgradeableAlbumArt(UpgradeableAlbumArtSignal {
                        key: upgrade_key,
                        data: UpgradeableAlbumArtData {
                            sidecar: primary_sidecar.clone(),
                            upgradeable_inodes,
                            upgradeable_paths,
                            embedded_format: emb_format,
                            embedded_width: emb_width,
                            embedded_height: emb_height,
                        },
                    }),
                ));
            }
        }
    }

    // Reconcile embeddable signals
    let (emb_cleared, emb_written, _, _) = reconcile_aggregate_signals::<EmbeddableAlbumArtSignal>(
        read_only_db,
        &sender,
        embeddable_computed,
        witness,
    );

    // Reconcile upgradeable signals
    let (upg_cleared, upg_written, _, _) = reconcile_aggregate_signals::<UpgradeableAlbumArtSignal>(
        read_only_db,
        &sender,
        upgradeable_computed,
        witness,
    );

    log_general(format!(
        "[COMPUTE] DetectEmbeddableAlbumArt: {} artless + {} with-art files across {} dirs, \
         {} dirs had sidecars, embeddable: {} signals ({} files) cleared={}, \
         upgradeable: {} signals ({} files) cleared={}",
        artless_files.len(),
        files_with_art.len(),
        all_dirs.len(),
        dirs_with_sidecar,
        emb_written,
        total_artless_emitted,
        emb_cleared,
        upg_written,
        total_upgradeable_emitted,
        upg_cleared,
    ));

    Result::success(computation, start.elapsed().as_millis() as u64, Vec::new())
}
