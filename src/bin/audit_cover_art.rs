//! One-off cover-art cleanup binary.
//!
//! Audits the corpus for poisoned/orphan sidecar cover images and stashes
//! them out of the corpus tree. Operates per-directory:
//!
//! 1. **Mismatch**: sidecar exists AND any audio sibling has embedded art
//!    AND the embedded vs sidecar dHash differ → stash sidecar.
//! 2. **No-embedded-anywhere**: sidecar exists AND all audio siblings have
//!    NO embedded art → stash sidecar.
//! 3. Sidecars in directories without audio siblings: left alone.
//!
//! Self-contained — does not link against the `mm` library (the package is a
//! binary crate). The dHash and Picture-extraction logic mirrors what lives
//! in `src/corpus/image_hash.rs` and `src/corpus/tags.rs`.
//!
//! ## Inputs
//!
//! Two TSVs dumped from the MM DB (`zone='corpus'`):
//! - `sidecars.tsv`:  `<inode>\t<zone_relative_path>` for `image_info.role='cover_front'`
//! - `audio.tsv`:     `<zone_relative_path>\t<has_pictures>` for all audio in corpus
//!
//! ## Operation
//!
//! - Default = dry-run; logs "would stash" decisions without moving anything.
//! - `--commit` = perform the moves into `<stash_root>/cover_art_audit/<preserved-rel-path>`.
//! - Collisions are resolved by appending `_` to the file stem (matches MM's
//!   `execute_move_to_stash` behavior in `src/meta/mutations/file_ops.rs`).
//!
//! Run from the host (NOT inside the container). Reads audio files for embedded
//! art extraction; writes nothing to audio. The DB is not touched — MM's watcher
//! reconciles `image_info` rows naturally when the sidecar inode disappears.

use std::collections::{BTreeMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;

// =============================================================================
// dHash-based perceptual visual match.
// Mirrors `src/corpus/image_hash.rs` exactly so behavior matches MM internals.
// =============================================================================

fn dhash(image_bytes: &[u8]) -> Result<u64> {
    use image::imageops::FilterType;
    let img = image::load_from_memory(image_bytes)?;
    let gray = img
        .grayscale()
        .resize_exact(9, 8, FilterType::Lanczos3)
        .to_luma8();
    let mut hash: u64 = 0;
    for y in 0..8u32 {
        for x in 0..8u32 {
            if gray.get_pixel(x, y).0[0] > gray.get_pixel(x + 1, y).0[0] {
                hash |= 1 << (y * 8 + x);
            }
        }
    }
    Ok(hash)
}

fn hamming_distance(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}

fn is_visual_match(existing_bytes: &[u8], candidate_bytes: &[u8]) -> Result<bool> {
    let h1 = dhash(existing_bytes)?;
    let h2 = dhash(candidate_bytes)?;
    Ok(hamming_distance(h1, h2) <= 10)
}

// =============================================================================
// Embedded picture extraction (lofty), format-aware to match MM behavior.
// =============================================================================

fn path_ext(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase())
        .unwrap_or_default()
}

fn pick_picture_bytes(pics: &[&lofty::picture::Picture]) -> Option<Vec<u8>> {
    use lofty::picture::PictureType;
    if pics.is_empty() {
        return None;
    }
    let pic = pics
        .iter()
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| pics.first())
        .copied()
        .unwrap();
    Some(pic.data().to_vec())
}

fn extract_first_picture_bytes(path: &Path) -> Option<Vec<u8>> {
    use lofty::config::ParseOptions;
    use lofty::file::AudioFile;
    use lofty::ogg::OggPictureStorage;

    let file = std::fs::File::open(path).ok()?;
    let mut reader = std::io::BufReader::new(file);

    match path_ext(path).as_str() {
        "flac" => {
            let flac =
                lofty::flac::FlacFile::read_from(&mut reader, ParseOptions::default()).ok()?;
            let mut all_pics: Vec<&lofty::picture::Picture> =
                flac.pictures().iter().map(|(p, _)| p).collect();
            if let Some(vc) = flac.vorbis_comments() {
                all_pics.extend(vc.pictures().iter().map(|(p, _)| p));
            }
            pick_picture_bytes(&all_pics)
        }
        "opus" => {
            let opus =
                lofty::ogg::OpusFile::read_from(&mut reader, ParseOptions::default()).ok()?;
            let pics: Vec<&lofty::picture::Picture> = opus
                .vorbis_comments()
                .pictures()
                .iter()
                .map(|(p, _)| p)
                .collect();
            pick_picture_bytes(&pics)
        }
        "ogg" => {
            let vorbis =
                lofty::ogg::VorbisFile::read_from(&mut reader, ParseOptions::default()).ok()?;
            let pics: Vec<&lofty::picture::Picture> = vorbis
                .vorbis_comments()
                .pictures()
                .iter()
                .map(|(p, _)| p)
                .collect();
            pick_picture_bytes(&pics)
        }
        "mp3" => {
            use lofty::id3::v2::Frame;
            let mp3 =
                lofty::mpeg::MpegFile::read_from(&mut reader, ParseOptions::default()).ok()?;
            let id3v2 = mp3.id3v2()?;
            let apic_pics: Vec<&lofty::picture::Picture> = id3v2
                .into_iter()
                .filter_map(|f| match f {
                    Frame::Picture(apic) => Some(&*apic.picture),
                    _ => None,
                })
                .collect();
            pick_picture_bytes(&apic_pics)
        }
        _ => {
            use lofty::file::TaggedFileExt;
            use lofty::probe::Probe;
            let tagged_file = Probe::open(path).ok().and_then(|p| p.read().ok())?;
            let pics: Vec<&lofty::picture::Picture> =
                tagged_file.tags().iter().flat_map(|t| t.pictures()).collect();
            pick_picture_bytes(&pics)
        }
    }
}

// =============================================================================
// Cleanup logic
// =============================================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StashReason {
    Mismatch,
    NoEmbeddedAnywhere,
}

impl StashReason {
    fn as_str(&self) -> &'static str {
        match self {
            StashReason::Mismatch => "mismatch",
            StashReason::NoEmbeddedAnywhere => "no_embedded_anywhere",
        }
    }
}

#[derive(Debug, Default)]
struct Stats {
    dirs_scanned: usize,
    sidecars_scanned: usize,
    stashed_mismatch: usize,
    stashed_no_embedded: usize,
    left_alone_no_audio: usize,
    left_alone_match: usize,
    errors_sidecar_unreadable: usize,
    errors_no_decodable_embedded: usize,
    errors_dhash: usize,
    errors_move: usize,
}

fn parent_dir(path: &str) -> Option<&str> {
    path.rfind('/').map(|i| &path[..i])
}

fn read_sidecars(path: &Path) -> Result<Vec<(i64, String)>> {
    let f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(f);
    let mut out = Vec::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        let inode_s = parts.next().unwrap_or("");
        let path_s = parts.next().unwrap_or("");
        let inode: i64 = inode_s
            .parse()
            .with_context(|| format!("bad inode at line {}", lineno + 1))?;
        if path_s.is_empty() {
            anyhow::bail!("empty path at line {}", lineno + 1);
        }
        out.push((inode, path_s.to_string()));
    }
    Ok(out)
}

/// Read audio.tsv and group by parent directory.
fn read_audio_by_dir(path: &Path) -> Result<BTreeMap<String, Vec<(String, bool)>>> {
    let f = fs::File::open(path).with_context(|| format!("open {}", path.display()))?;
    let reader = BufReader::new(f);
    let mut by_dir: BTreeMap<String, Vec<(String, bool)>> = BTreeMap::new();
    for (lineno, line) in reader.lines().enumerate() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        let mut parts = line.splitn(2, '\t');
        let path_s = parts.next().unwrap_or("");
        let has_pics_s = parts.next().unwrap_or("0");
        if path_s.is_empty() {
            anyhow::bail!("empty audio path at line {}", lineno + 1);
        }
        let has_pictures = has_pics_s.trim() == "1";
        let dir = match parent_dir(path_s) {
            Some(d) => d.to_string(),
            None => continue,
        };
        by_dir
            .entry(dir)
            .or_default()
            .push((path_s.to_string(), has_pictures));
    }
    Ok(by_dir)
}

fn corpus_abs(corpus_root: &Path, zone_rel: &str) -> PathBuf {
    corpus_root.join(zone_rel)
}

/// Build the stash destination, applying the underscore-stem collision policy
/// used by MM's `execute_move_to_stash`.
fn build_stash_dest(stash_root: &Path, stash_name: &str, zone_rel: &str) -> PathBuf {
    let mut dest = stash_root.join(stash_name).join(zone_rel);
    if !dest.exists() {
        return dest;
    }
    let parent = dest.parent().map(PathBuf::from);
    let stem = dest
        .file_stem()
        .map(|s| s.to_os_string())
        .unwrap_or_default();
    let extension = dest.extension().map(|e| e.to_os_string());
    let mut new_stem: OsString = stem;
    loop {
        new_stem.push("_");
        let mut new_filename = new_stem.clone();
        if let Some(ref ext) = extension {
            new_filename.push(".");
            new_filename.push(ext);
        }
        dest = match &parent {
            Some(p) => p.join(&new_filename),
            None => PathBuf::from(&new_filename),
        };
        if !dest.exists() {
            return dest;
        }
    }
}

#[derive(Debug)]
enum Decision {
    Stash(StashReason),
    LeaveNoAudio,
    LeaveMatch,
    ErrSidecarUnreadable(anyhow::Error),
    ErrNoDecodableEmbedded,
    ErrDhash(anyhow::Error),
}

fn decide(
    corpus_root: &Path,
    sidecar_zone_rel: &str,
    audio_siblings: Option<&Vec<(String, bool)>>,
) -> Decision {
    let Some(siblings) = audio_siblings else {
        return Decision::LeaveNoAudio;
    };
    if siblings.is_empty() {
        return Decision::LeaveNoAudio;
    }

    let with_pics: Vec<&String> = siblings
        .iter()
        .filter_map(|(p, has)| if *has { Some(p) } else { None })
        .collect();

    if with_pics.is_empty() {
        return Decision::Stash(StashReason::NoEmbeddedAnywhere);
    }

    let sidecar_abs = corpus_abs(corpus_root, sidecar_zone_rel);
    let sidecar_bytes = match fs::read(&sidecar_abs) {
        Ok(b) => b,
        Err(e) => {
            return Decision::ErrSidecarUnreadable(
                anyhow::Error::new(e).context(format!("read {}", sidecar_abs.display())),
            )
        }
    };

    // Cap at 3 sibling reads — embedded art is consistent within an album.
    // Reading more wastes time without changing the verdict.
    const MAX_SIBLING_READS: usize = 3;
    let mut at_least_one_decoded = false;
    let mut any_match = false;
    let mut decoded = 0;
    for sib in &with_pics {
        if decoded >= MAX_SIBLING_READS {
            break;
        }
        let sib_abs = corpus_abs(corpus_root, sib);
        let Some(emb) = extract_first_picture_bytes(&sib_abs) else {
            continue;
        };
        at_least_one_decoded = true;
        decoded += 1;
        match is_visual_match(&sidecar_bytes, &emb) {
            Ok(true) => {
                any_match = true;
                break;
            }
            Ok(false) => {}
            Err(e) => return Decision::ErrDhash(e),
        }
    }

    if !at_least_one_decoded {
        return Decision::ErrNoDecodableEmbedded;
    }
    if any_match {
        Decision::LeaveMatch
    } else {
        Decision::Stash(StashReason::Mismatch)
    }
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let mut sidecars_path = PathBuf::from("/tmp/cover_audit/sidecars.tsv");
    let mut audio_path = PathBuf::from("/tmp/cover_audit/audio.tsv");
    let mut corpus_root = PathBuf::from("/mnt/pool/library/archive/audio");
    let mut stash_root = PathBuf::from("/mnt/pool/library/archive/stash");
    let stash_name = "cover_art_audit".to_string();
    let mut commit = false;
    let mut limit: Option<usize> = None;
    let mut sample_filter: Option<String> = None;
    let mut debug_pair: Option<(String, String)> = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--commit" => commit = true,
            "--sidecars" => {
                i += 1;
                sidecars_path = PathBuf::from(&args[i]);
            }
            "--audio" => {
                i += 1;
                audio_path = PathBuf::from(&args[i]);
            }
            "--corpus-root" => {
                i += 1;
                corpus_root = PathBuf::from(&args[i]);
            }
            "--stash-root" => {
                i += 1;
                stash_root = PathBuf::from(&args[i]);
            }
            "--limit" => {
                i += 1;
                limit = Some(args[i].parse().context("--limit")?);
            }
            "--filter" => {
                i += 1;
                sample_filter = Some(args[i].clone());
            }
            "--debug-pair" => {
                i += 1;
                let sidecar = args[i].clone();
                i += 1;
                let audio = args[i].clone();
                debug_pair = Some((sidecar, audio));
            }
            "--help" | "-h" => {
                print_help();
                return Ok(());
            }
            other => {
                eprintln!("unknown arg: {}", other);
                print_help();
                std::process::exit(2);
            }
        }
        i += 1;
    }

    if let Some((sidecar, audio)) = debug_pair {
        let sidecar_bytes = fs::read(&sidecar)
            .with_context(|| format!("read sidecar {}", sidecar))?;
        let audio_path = PathBuf::from(&audio);
        let emb = extract_first_picture_bytes(&audio_path)
            .ok_or_else(|| anyhow::anyhow!("no decodable embedded picture in {}", audio))?;
        let h1 = dhash(&sidecar_bytes)?;
        let h2 = dhash(&emb)?;
        let dist = hamming_distance(h1, h2);
        println!("sidecar:  {} ({} bytes) dhash={:016x}", sidecar, sidecar_bytes.len(), h1);
        println!("embedded: {} ({} bytes) dhash={:016x}", audio, emb.len(), h2);
        println!("hamming distance: {}  (threshold ≤10)", dist);
        println!("verdict: {}", if dist <= 10 { "MATCH" } else { "DIFFERENT" });
        return Ok(());
    }

    eprintln!("cover-art audit");
    eprintln!("  sidecars:    {}", sidecars_path.display());
    eprintln!("  audio:       {}", audio_path.display());
    eprintln!("  corpus root: {}", corpus_root.display());
    eprintln!("  stash root:  {}", stash_root.display());
    eprintln!("  stash name:  {}", stash_name);
    eprintln!("  commit:      {}", commit);
    if let Some(l) = limit {
        eprintln!("  limit:       {}", l);
    }
    if let Some(f) = &sample_filter {
        eprintln!("  filter:      {}", f);
    }
    eprintln!();

    let mut sidecars = read_sidecars(&sidecars_path)?;
    let audio_by_dir = read_audio_by_dir(&audio_path)?;
    eprintln!(
        "loaded {} sidecars, {} audio dirs",
        sidecars.len(),
        audio_by_dir.len()
    );

    let mut stats = Stats::default();
    let mut stash_samples: Vec<(String, StashReason)> = Vec::new();
    let mut error_samples: Vec<(String, String)> = Vec::new();

    sidecars.sort_by(|a, b| a.1.cmp(&b.1));

    // Filter + limit first.
    let work: Vec<&(i64, String)> = sidecars
        .iter()
        .filter(|(_, p)| {
            if let Some(f) = &sample_filter {
                p.contains(f)
            } else {
                true
            }
        })
        .take(limit.unwrap_or(usize::MAX))
        .collect();

    eprintln!("processing {} sidecars in parallel...", work.len());

    let progress = AtomicUsize::new(0);
    let total = work.len();

    // Parallel decision phase: pure compute + reads, no mutations.
    let decisions: Vec<(String, Decision)> = work
        .par_iter()
        .map(|(_, sidecar_path)| {
            let dir = parent_dir(sidecar_path).map(|s| s.to_string());
            let decision = match dir {
                Some(d) => decide(&corpus_root, sidecar_path, audio_by_dir.get(&d)),
                None => Decision::LeaveNoAudio,
            };
            let n = progress.fetch_add(1, Ordering::Relaxed) + 1;
            if n % 500 == 0 || n == total {
                eprintln!("  progress: {}/{} sidecars", n, total);
            }
            (sidecar_path.clone(), decision)
        })
        .collect();

    // Serial action phase: print, stash, accumulate stats.
    let mut dirs_seen: HashSet<String> = HashSet::new();
    for (sidecar_path, decision) in decisions {
        stats.sidecars_scanned += 1;
        if let Some(d) = parent_dir(&sidecar_path) {
            dirs_seen.insert(d.to_string());
        }

        match decision {
            Decision::Stash(reason) => {
                let abs = corpus_abs(&corpus_root, &sidecar_path);
                if !commit {
                    println!("DRY {} {}", reason.as_str(), sidecar_path);
                    if stash_samples.len() < 30 {
                        stash_samples.push((sidecar_path.clone(), reason));
                    }
                } else {
                    let dest = build_stash_dest(&stash_root, &stash_name, &sidecar_path);
                    if let Some(parent) = dest.parent() {
                        if let Err(e) = fs::create_dir_all(parent) {
                            stats.errors_move += 1;
                            eprintln!("ERR mkdir {}: {}", parent.display(), e);
                            continue;
                        }
                    }
                    match fs::rename(&abs, &dest) {
                        Ok(()) => {
                            println!(
                                "STASH {} {} -> {}",
                                reason.as_str(),
                                abs.display(),
                                dest.display()
                            );
                            if stash_samples.len() < 30 {
                                stash_samples.push((sidecar_path.clone(), reason));
                            }
                        }
                        Err(e) => {
                            stats.errors_move += 1;
                            eprintln!("ERR mv {} -> {}: {}", abs.display(), dest.display(), e);
                            continue;
                        }
                    }
                }
                match reason {
                    StashReason::Mismatch => stats.stashed_mismatch += 1,
                    StashReason::NoEmbeddedAnywhere => stats.stashed_no_embedded += 1,
                }
            }
            Decision::LeaveNoAudio => {
                stats.left_alone_no_audio += 1;
            }
            Decision::LeaveMatch => {
                stats.left_alone_match += 1;
            }
            Decision::ErrSidecarUnreadable(e) => {
                stats.errors_sidecar_unreadable += 1;
                let msg = format!("sidecar unreadable: {}: {:#}", sidecar_path, e);
                eprintln!("ERR {}", msg);
                if error_samples.len() < 30 {
                    error_samples.push((sidecar_path.clone(), msg));
                }
            }
            Decision::ErrNoDecodableEmbedded => {
                stats.errors_no_decodable_embedded += 1;
                let msg = format!(
                    "no decodable embedded picture (DB said has_pictures): {}",
                    sidecar_path
                );
                eprintln!("ERR {}", msg);
                if error_samples.len() < 30 {
                    error_samples.push((sidecar_path.clone(), msg));
                }
            }
            Decision::ErrDhash(e) => {
                stats.errors_dhash += 1;
                let msg = format!("dhash failed for {}: {:#}", sidecar_path, e);
                eprintln!("ERR {}", msg);
                if error_samples.len() < 30 {
                    error_samples.push((sidecar_path.clone(), msg));
                }
            }
        }
    }
    stats.dirs_scanned = dirs_seen.len();

    eprintln!();
    eprintln!("=== summary ===");
    eprintln!("commit:                       {}", commit);
    eprintln!("dirs scanned:                 {}", stats.dirs_scanned);
    eprintln!("sidecars scanned:             {}", stats.sidecars_scanned);
    eprintln!("stashed (mismatch):           {}", stats.stashed_mismatch);
    eprintln!("stashed (no_embedded_any):    {}", stats.stashed_no_embedded);
    eprintln!("left alone (matched):         {}", stats.left_alone_match);
    eprintln!("left alone (no audio in dir): {}", stats.left_alone_no_audio);
    eprintln!("errors:");
    eprintln!("  sidecar unreadable:         {}", stats.errors_sidecar_unreadable);
    eprintln!("  no decodable embedded:      {}", stats.errors_no_decodable_embedded);
    eprintln!("  dhash failed:               {}", stats.errors_dhash);
    eprintln!("  mv failed:                  {}", stats.errors_move);

    if !stash_samples.is_empty() {
        eprintln!();
        eprintln!("first {} stash samples:", stash_samples.len().min(15));
        for (p, r) in stash_samples.iter().take(15) {
            eprintln!("  [{}] {}", r.as_str(), p);
        }
    }
    if !error_samples.is_empty() {
        eprintln!();
        eprintln!("first {} error samples:", error_samples.len().min(15));
        for (p, m) in error_samples.iter().take(15) {
            eprintln!("  [{}] {}", p, m);
        }
    }

    Ok(())
}

fn print_help() {
    eprintln!("audit_cover_art — one-off cover-art cleanup");
    eprintln!();
    eprintln!("Usage: audit_cover_art [OPTIONS]");
    eprintln!();
    eprintln!("Options:");
    eprintln!("  --sidecars PATH      sidecars TSV (inode\\tpath) [default: /tmp/cover_audit/sidecars.tsv]");
    eprintln!("  --audio PATH         audio TSV (path\\thas_pictures) [default: /tmp/cover_audit/audio.tsv]");
    eprintln!("  --corpus-root PATH   abs corpus root [default: /mnt/pool/library/archive/audio]");
    eprintln!("  --stash-root PATH    abs stash root [default: /mnt/pool/library/archive/stash]");
    eprintln!("  --commit             actually move files (default = dry-run)");
    eprintln!("  --limit N            process only first N sidecars (sorted)");
    eprintln!("  --filter SUBSTR      only sidecars whose path contains SUBSTR");
}
