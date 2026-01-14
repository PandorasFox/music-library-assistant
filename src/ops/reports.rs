//! Report Module
//!
//! Part of MLA's toolkit: provides the "report" capability for analysis.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::config;
use crate::corpus::db::{Database, Track};
use crate::corpus::deduplication::{
    compare_track_quality, durations_within_tolerance, is_same_album_different_tracks,
    QualityVerdict,
};

const DURATION_TOLERANCE_MS: i64 = 2000; // 2 seconds tolerance for duration matching

#[derive(Debug, Clone)]
pub struct LegacyMatch {
    pub legacy_path: String,
    pub corpus_path: String,
    pub match_type: MatchType,
    pub confidence: MatchConfidence,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MatchType {
    ExactMetadata,       // Artist, album, title all match
    MetadataAndDuration, // Metadata + duration match
    DurationAndFileType, // Only duration and file type match (lower confidence)
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum MatchConfidence {
    High,
    Medium,
    Low,
}

/// Generate a report of legacy library files that definitively exist in the corpus
pub fn generate_legacy_report(output_path: &Path) -> Result<String> {
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;

    // Get all legacy and corpus tracks
    let legacy_tracks = db
        .get_all_tracks(Some("legacy"))
        .context("Failed to load legacy tracks")?;

    if legacy_tracks.is_empty() {
        return Ok("No legacy library scanned. Please scan your legacy library first with: mla scan <path> --name legacy".to_string());
    }

    let corpus_tracks = db
        .get_all_tracks(Some("corpus"))
        .context("Failed to load corpus tracks")?;

    if corpus_tracks.is_empty() {
        return Ok(
            "No corpus scanned. Please scan your corpus first with: mla scan <path> --name corpus"
                .to_string(),
        );
    }

    // Build index of corpus tracks for faster lookups
    let mut corpus_by_metadata: HashMap<String, Vec<&Track>> = HashMap::new();
    let mut corpus_by_duration: HashMap<i64, Vec<&Track>> = HashMap::new();

    for track in &corpus_tracks {
        // Index by metadata
        if let (Some(artist), Some(album), Some(title)) =
            (&track.artist, &track.album, &track.title)
        {
            let key = format!(
                "{}|{}|{}",
                normalize_string(artist),
                normalize_string(album),
                normalize_string(title)
            );
            corpus_by_metadata.entry(key).or_default().push(track);
        }

        // Index by duration
        if let Some(dur) = track.duration_ms {
            corpus_by_duration.entry(dur).or_default().push(track);
        }
    }

    // Find matches
    let mut matches = Vec::new();

    for legacy_track in &legacy_tracks {
        let mut found_matches =
            find_matches(legacy_track, &corpus_by_metadata, &corpus_by_duration);
        matches.append(&mut found_matches);
    }

    // Sort by confidence and path
    matches.sort_by(|a, b| {
        b.confidence
            .cmp(&a.confidence)
            .then_with(|| a.legacy_path.cmp(&b.legacy_path))
    });

    // Write report
    let mut file = File::create(output_path).context("Failed to create report file")?;

    writeln!(
        file,
        "Music Library Assistant - Legacy Library Match Report"
    )?;
    writeln!(
        file,
        "======================================================\n"
    )?;
    writeln!(file, "Legacy tracks scanned: {}", legacy_tracks.len())?;
    writeln!(file, "Corpus tracks scanned: {}", corpus_tracks.len())?;
    writeln!(file, "Matches found: {}\n", matches.len())?;

    let high_confidence = matches
        .iter()
        .filter(|m| m.confidence == MatchConfidence::High)
        .count();
    let medium_confidence = matches
        .iter()
        .filter(|m| m.confidence == MatchConfidence::Medium)
        .count();
    let low_confidence = matches
        .iter()
        .filter(|m| m.confidence == MatchConfidence::Low)
        .count();

    writeln!(file, "Match confidence breakdown:")?;
    writeln!(file, "  High confidence: {}", high_confidence)?;
    writeln!(file, "  Medium confidence: {}", medium_confidence)?;
    writeln!(file, "  Low confidence: {}\n", low_confidence)?;

    writeln!(file, "{}", "=".repeat(80))?;
    writeln!(file, "\nDETAILED MATCHES\n")?;

    for match_entry in &matches {
        let confidence_str = match match_entry.confidence {
            MatchConfidence::High => "HIGH",
            MatchConfidence::Medium => "MED ",
            MatchConfidence::Low => "LOW ",
        };

        let match_type_str = match match_entry.match_type {
            MatchType::ExactMetadata => "Exact metadata match",
            MatchType::MetadataAndDuration => "Metadata + duration match",
            MatchType::DurationAndFileType => "Duration + file type match (tags may differ)",
        };

        writeln!(file, "[{}] {}", confidence_str, match_type_str)?;
        writeln!(file, "Legacy:  {}", match_entry.legacy_path)?;
        writeln!(file, "Corpus: {}", match_entry.corpus_path)?;
        writeln!(file)?;
    }

    writeln!(file, "{}", "=".repeat(80))?;
    writeln!(file, "\nPARSABLE FORMAT (for scripting)\n")?;

    for match_entry in &matches {
        if match_entry.confidence == MatchConfidence::High {
            writeln!(
                file,
                "{}|{}",
                match_entry.legacy_path, match_entry.corpus_path
            )?;
        }
    }

    Ok(format!(
        "Legacy library report generated: {}\n  Total matches: {} ({} high confidence)",
        output_path.display(),
        matches.len(),
        high_confidence
    ))
}

fn find_matches<'a>(
    legacy_track: &Track,
    corpus_by_metadata: &HashMap<String, Vec<&'a Track>>,
    corpus_by_duration: &HashMap<i64, Vec<&'a Track>>,
) -> Vec<LegacyMatch> {
    let mut matches = Vec::new();

    // Try exact metadata match
    if let (Some(artist), Some(album), Some(title)) = (
        &legacy_track.artist,
        &legacy_track.album,
        &legacy_track.title,
    ) {
        let key = format!(
            "{}|{}|{}",
            normalize_string(artist),
            normalize_string(album),
            normalize_string(title)
        );

        if let Some(archive_matches) = corpus_by_metadata.get(&key) {
            for corpus_track in archive_matches {
                // Check if duration also matches (if available)
                let duration_match = if let (Some(leg_dur), Some(corp_dur)) =
                    (legacy_track.duration_ms, corpus_track.duration_ms)
                {
                    (leg_dur - corp_dur).abs() <= DURATION_TOLERANCE_MS
                } else {
                    false
                };

                let (match_type, confidence) = if duration_match {
                    (MatchType::MetadataAndDuration, MatchConfidence::High)
                } else {
                    (MatchType::ExactMetadata, MatchConfidence::High)
                };

                matches.push(LegacyMatch {
                    legacy_path: legacy_track.path.clone(),
                    corpus_path: corpus_track.path.clone(),
                    match_type,
                    confidence,
                });
            }
        }
    }

    // If no metadata match, try duration + file type matching
    if matches.is_empty() {
        if let Some(legacy_dur) = legacy_track.duration_ms {
            // Check durations within tolerance
            for offset in -DURATION_TOLERANCE_MS..=DURATION_TOLERANCE_MS {
                let check_dur = legacy_dur + offset;
                if let Some(archive_matches) = corpus_by_duration.get(&check_dur) {
                    for corpus_track in archive_matches {
                        if corpus_track.file_type == legacy_track.file_type {
                            matches.push(LegacyMatch {
                                legacy_path: legacy_track.path.clone(),
                                corpus_path: corpus_track.path.clone(),
                                match_type: MatchType::DurationAndFileType,
                                confidence: MatchConfidence::Low,
                            });
                        }
                    }
                }
            }
        }
    }

    matches
}

fn normalize_string(s: &str) -> String {
    s.to_lowercase().trim().to_string()
}

/// Generate a report of corpus directories and their deployment status
pub fn generate_deployment_report(output_path: &Path) -> Result<String> {
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;

    let corpus_tracks = db
        .get_all_tracks(Some("corpus"))
        .context("Failed to load corpus tracks")?;

    if corpus_tracks.is_empty() {
        return Ok("No corpus scanned. Please scan your corpus first.".to_string());
    }

    // Get all library tracks (any source that isn't 'corpus' or 'legacy')
    let sources = db.get_sources()?;
    let library_sources: Vec<String> = sources
        .iter()
        .filter(|s| *s != "corpus" && *s != "legacy")
        .cloned()
        .collect();

    let mut library_inodes = std::collections::HashSet::new();
    for source in &library_sources {
        let tracks = db.get_all_tracks(Some(source))?;
        for track in tracks {
            library_inodes.insert(track.inode);
        }
    }

    // Group corpus files by directory
    let mut dir_stats: HashMap<String, (usize, usize)> = HashMap::new();

    for track in &corpus_tracks {
        if let Some(parent) = Path::new(&track.path).parent() {
            let dir_path = parent.to_string_lossy().to_string();
            let (total, deployed) = dir_stats.entry(dir_path).or_insert((0, 0));
            *total += 1;
            if library_inodes.contains(&track.inode) {
                *deployed += 1;
            }
        }
    }

    // Write report
    let mut file = File::create(output_path)?;

    writeln!(file, "Music Library Assistant - Corpus Deployment Report")?;
    writeln!(
        file,
        "===================================================\n"
    )?;

    writeln!(file, "Corpus tracks: {}", corpus_tracks.len())?;
    writeln!(file, "Library sources scanned: {:?}\n", library_sources)?;

    let mut dirs: Vec<_> = dir_stats.iter().collect();
    dirs.sort_by_key(|(path, _)| path.as_str());

    writeln!(file, "\nDIRECTORY DEPLOYMENT STATUS\n")?;

    for (dir, (total, deployed)) in &dirs {
        let pct = if *total > 0 {
            (*deployed as f64 / *total as f64) * 100.0
        } else {
            0.0
        };
        let status = if *deployed == 0 {
            "NOT DEPLOYED"
        } else if *deployed == *total {
            "FULLY DEPLOYED"
        } else {
            "PARTIALLY DEPLOYED"
        };

        writeln!(
            file,
            "{:<15} {}/{} files ({:.0}%)",
            status, deployed, total, pct
        )?;
        writeln!(file, "  {}", dir)?;
        writeln!(file)?;
    }

    Ok(format!(
        "Deployment report generated: {}",
        output_path.display()
    ))
}

/// Generate a report of quality issues (canonicalization, tagging consistency)
pub fn generate_quality_report(output_path: &Path) -> Result<String> {
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;

    // Only analyze corpus source for quality issues
    let all_tracks = db.get_all_tracks(Some("corpus"))?;

    if all_tracks.is_empty() {
        return Ok("No corpus scanned. Please scan your corpus first.".to_string());
    }

    // Find canonicalization issues
    let mut artist_variants: HashMap<String, Vec<String>> = HashMap::new();
    let mut album_artist_issues: HashMap<String, Vec<String>> = HashMap::new();

    for track in &all_tracks {
        if let Some(artist) = &track.artist {
            let normalized = normalize_string(artist);
            artist_variants
                .entry(normalized)
                .or_default()
                .push(artist.clone());
        }

        // Check for missing album_artist when artist exists
        if track.artist.is_some() && track.album_artist.is_none() {
            if let Some(album) = &track.album {
                album_artist_issues
                    .entry(album.clone())
                    .or_default()
                    .push(track.path.clone());
            }
        }
    }

    // Filter to only show issues (multiple variants of same name)
    let mut canonicalization_issues: Vec<_> = artist_variants
        .iter()
        .filter(|(_, variants)| {
            let unique: std::collections::HashSet<_> = variants.iter().collect();
            unique.len() > 1
        })
        .collect();
    canonicalization_issues.sort_by_key(|(name, _)| name.as_str());

    // Write report
    let mut file = File::create(output_path)?;

    writeln!(file, "Music Library Assistant - Quality Report")?;
    writeln!(file, "=========================================\n")?;

    writeln!(file, "Archive tracks analyzed: {}\n", all_tracks.len())?;
    writeln!(file, "Note: This report analyzes only corpus tracks.\n")?;

    writeln!(file, "ARTIST NAME CANONICALIZATION ISSUES")?;
    writeln!(file, "------------------------------------\n")?;

    if canonicalization_issues.is_empty() {
        writeln!(file, "No issues found.\n")?;
    } else {
        for (normalized, variants) in &canonicalization_issues {
            let unique: std::collections::HashSet<_> = variants.iter().collect();
            writeln!(file, "Normalized: {}", normalized)?;
            writeln!(file, "Variants found:")?;
            for variant in unique {
                let count = variants.iter().filter(|v| *v == variant).count();
                writeln!(file, "  - \"{}\" ({} occurrences)", variant, count)?;
            }
            writeln!(file)?;
        }
    }

    writeln!(file, "\nMISSING ALBUM ARTIST TAGS")?;
    writeln!(file, "-------------------------\n")?;

    if album_artist_issues.is_empty() {
        writeln!(file, "No issues found.\n")?;
    } else {
        let mut albums: Vec<_> = album_artist_issues.iter().collect();
        albums.sort_by_key(|(album, _)| album.as_str());

        for (album, paths) in albums {
            writeln!(file, "Album: {}", album)?;
            writeln!(file, "  {} tracks missing album_artist tag", paths.len())?;
            writeln!(file, "  Example: {}", paths[0])?;
            writeln!(file)?;
        }
    }

    Ok(format!("Quality report generated: {}\n  Canonicalization issues: {}\n  Missing album_artist: {} albums",
        output_path.display(), canonicalization_issues.len(), album_artist_issues.len()))
}

/// Generate a report of duplicate tracks within the corpus
pub fn generate_duplicate_report(output_path: &Path) -> Result<String> {
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;
    let _cfg = config::load_config()?;

    // Only analyze corpus source for duplicates
    let all_tracks = db.get_all_tracks(Some("corpus"))?;

    if all_tracks.is_empty() {
        return Ok("No corpus scanned. Please scan your corpus first.".to_string());
    }

    // === METADATA-BASED DUPLICATE DETECTION ===
    // Use ISRC to intelligently filter out re-releases

    // Group tracks by metadata
    let mut track_groups: HashMap<String, Vec<&Track>> = HashMap::new();

    for track in &all_tracks {
        // Use album_artist if available, fallback to artist
        // This is more reliable for compilation albums
        let artist_for_matching = track.album_artist.as_ref().or(track.artist.as_ref());

        if let (Some(artist), Some(album), Some(title)) =
            (artist_for_matching, &track.album, &track.title)
        {
            // Skip remixes and deluxe editions for now (basic heuristic)
            let title_lower = title.to_lowercase();
            let album_lower = album.to_lowercase();
            if title_lower.contains("remix")
                || title_lower.contains("edit")
                || album_lower.contains("deluxe")
                || album_lower.contains("edition")
            {
                continue;
            }

            let key = format!(
                "{}|{}|{}",
                normalize_string(artist),
                normalize_string(album),
                normalize_string(title)
            );
            track_groups.entry(key).or_default().push(track);
        }
    }

    // Filter to only duplicates (tracks appearing in multiple locations within corpus)
    // Use ISRC to filter out legitimate re-releases
    let mut metadata_duplicates: Vec<_> = track_groups
        .iter()
        .filter(|(_, tracks)| {
            if tracks.len() <= 1 {
                return false;
            }

            // Check if these are legitimate re-releases using ISRC
            // If tracks have same ISRC but different albums, they're legitimate re-releases
            let isrcs: Vec<_> = tracks.iter().filter_map(|t| t.isrc.as_ref()).collect();

            // If all tracks have ISRC codes
            if isrcs.len() == tracks.len() && !isrcs.is_empty() {
                // Check if they all have the same ISRC
                let first_isrc = isrcs[0];
                if isrcs.iter().all(|isrc| *isrc == first_isrc) {
                    // Same ISRC - check if they're on different albums
                    let albums: std::collections::HashSet<_> =
                        tracks.iter().filter_map(|t| t.album.as_ref()).collect();

                    // If they're on different albums, this is a legitimate re-release
                    if albums.len() > 1 {
                        return false; // Not a duplicate, just a re-release
                    }
                }
            }

            // Either no ISRC info, or same ISRC + same album - potential duplicate
            true
        })
        .collect();

    metadata_duplicates.sort_by_key(|(key, _)| key.as_str());

    // === FINGERPRINT-BASED DUPLICATE DETECTION ===

    // Group tracks by acoustic fingerprint
    let mut fingerprint_groups: HashMap<String, Vec<&Track>> = HashMap::new();
    let mut tracks_with_fingerprints = 0;

    for track in &all_tracks {
        if let Some(fp) = &track.fingerprint {
            if !fp.is_empty() {
                tracks_with_fingerprints += 1;
                fingerprint_groups
                    .entry(fp.clone())
                    .or_default()
                    .push(track);
            }
        }
    }

    // Filter to only duplicates, applying false-positive filters
    let mut filtered_same_album = 0usize;
    let mut filtered_duration = 0usize;

    let mut fingerprint_duplicates: Vec<_> = fingerprint_groups
        .iter()
        .filter(|(_, tracks)| {
            if tracks.len() < 2 {
                return false;
            }

            // Convert to owned tracks for filter functions
            let owned_tracks: Vec<Track> = tracks.iter().map(|t| (*t).clone()).collect();

            // Filter 1: Skip if these are different tracks from the same album
            if is_same_album_different_tracks(&owned_tracks) {
                filtered_same_album += 1;
                return false;
            }

            // Filter 2: Skip if duration variance exceeds 10%
            if !durations_within_tolerance(&owned_tracks, 0.10) {
                filtered_duration += 1;
                return false;
            }

            true
        })
        .collect();

    fingerprint_duplicates.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    // Categorize by quality (auto-resolvable vs manual review needed)
    let mut auto_resolvable: Vec<(&String, usize, &Track, Vec<&Track>)> = Vec::new();
    let mut manual_review: Vec<(&String, &Vec<&Track>)> = Vec::new();

    for (fp, tracks) in &fingerprint_duplicates {
        let owned_tracks: Vec<Track> = tracks.iter().map(|t| (*t).clone()).collect();
        match compare_track_quality(&owned_tracks) {
            QualityVerdict::ClearWinner(idx) => {
                let winner = tracks[idx];
                let losers: Vec<_> = tracks
                    .iter()
                    .enumerate()
                    .filter(|(i, _)| *i != idx)
                    .map(|(_, t)| *t)
                    .collect();
                auto_resolvable.push((*fp, idx, winner, losers));
            }
            _ => {
                manual_review.push((*fp, *tracks));
            }
        }
    }

    // Write report
    let mut file = File::create(output_path)?;

    writeln!(file, "Music Library Assistant - Duplicate Detection Report")?;
    writeln!(
        file,
        "====================================================\n"
    )?;

    writeln!(file, "Corpus tracks analyzed: {}", all_tracks.len())?;
    writeln!(file, "Note: This report analyzes only corpus tracks.\n")?;
    writeln!(
        file,
        "ISRC-based filtering: Tracks with same ISRC on different albums are"
    )?;
    writeln!(
        file,
        "considered legitimate re-releases and excluded from this report.\n"
    )?;
    writeln!(
        file,
        "Tracks with fingerprints: {}",
        tracks_with_fingerprints
    )?;
    writeln!(
        file,
        "Metadata-based duplicate groups: {}",
        metadata_duplicates.len()
    )?;

    // Fingerprint duplicate stats with filtering breakdown
    writeln!(file, "\nFingerprint-based duplicates:")?;
    writeln!(
        file,
        "  Total groups after filtering: {}",
        fingerprint_duplicates.len()
    )?;
    writeln!(
        file,
        "  Filtered out (same album, different tracks): {}",
        filtered_same_album
    )?;
    writeln!(
        file,
        "  Filtered out (duration variance >10%): {}",
        filtered_duration
    )?;
    writeln!(
        file,
        "  Auto-resolvable (clear quality winner): {}",
        auto_resolvable.len()
    )?;
    writeln!(
        file,
        "  Manual review needed: {}\n",
        manual_review.len()
    )?;

    // === METADATA DUPLICATES SECTION ===

    writeln!(file, "\n{}", "=".repeat(80))?;
    writeln!(file, "METADATA-BASED DUPLICATES")?;
    writeln!(file, "{}\n", "=".repeat(80))?;

    if metadata_duplicates.is_empty() {
        writeln!(file, "No metadata-based duplicates found.\n")?;
    } else {
        for (key, tracks) in &metadata_duplicates {
            writeln!(file, "Track: {}", key.replace('|', " - "))?;
            writeln!(file, "Instances: {}", tracks.len())?;

            for track in *tracks {
                let bitrate_str = track
                    .bitrate_kbps
                    .map(|br| format!("{}kbps", br))
                    .unwrap_or_else(|| "unknown".to_string());
                let fp_status = if track.fingerprint.is_some() {
                    "FP"
                } else {
                    "  "
                };
                let isrc_str = track
                    .isrc
                    .as_ref()
                    .map(|s| format!("ISRC:{}", s))
                    .unwrap_or_else(|| "no-ISRC".to_string());
                writeln!(
                    file,
                    "  [{}] [{}] [{}] [{}] {} - {}",
                    fp_status,
                    isrc_str,
                    track.source,
                    track.file_type.to_uppercase(),
                    bitrate_str,
                    track.path
                )?;
            }
            writeln!(file)?;
        }
    }

    // === FINGERPRINT DUPLICATES SECTION ===

    writeln!(file, "\n{}", "=".repeat(80))?;
    writeln!(file, "FINGERPRINT-BASED DUPLICATES")?;
    writeln!(file, "{}", "=".repeat(80))?;
    writeln!(
        file,
        "(Same audio content, potentially different formats/tags)\n"
    )?;

    if fingerprint_duplicates.is_empty() {
        writeln!(file, "No fingerprint-based duplicates found.\n")?;
    } else {
        // === AUTO-RESOLVABLE DUPLICATES (clear quality winner) ===
        if !auto_resolvable.is_empty() {
            writeln!(file, "\n{}", "-".repeat(60))?;
            writeln!(file, "AUTO-RESOLVABLE (clear quality winner)")?;
            writeln!(file, "{}", "-".repeat(60))?;
            writeln!(
                file,
                "These can be safely resolved by keeping the higher-quality version.\n"
            )?;

            for (fp_hash, _winner_idx, winner, losers) in &auto_resolvable {
                let track_info =
                    if let (Some(artist), Some(title)) = (&winner.artist, &winner.title) {
                        format!("{} - {}", artist, title)
                    } else {
                        "Unknown track".to_string()
                    };

                writeln!(file, "Track: {}", track_info)?;
                writeln!(
                    file,
                    "Fingerprint: {}...",
                    &fp_hash.chars().take(40).collect::<String>()
                )?;

                // Show winner
                let winner_bitrate = winner
                    .bitrate_kbps
                    .map(|br| format!("{}kbps", br))
                    .unwrap_or_else(|| "unknown".to_string());
                writeln!(
                    file,
                    "  KEEP: [{}] {} - {}",
                    winner.file_type.to_uppercase(),
                    winner_bitrate,
                    winner.path
                )?;

                // Show losers
                for loser in losers {
                    let loser_bitrate = loser
                        .bitrate_kbps
                        .map(|br| format!("{}kbps", br))
                        .unwrap_or_else(|| "unknown".to_string());
                    writeln!(
                        file,
                        "  DROP: [{}] {} - {}",
                        loser.file_type.to_uppercase(),
                        loser_bitrate,
                        loser.path
                    )?;
                }
                writeln!(file)?;
            }
        }

        // === MANUAL REVIEW DUPLICATES ===
        if !manual_review.is_empty() {
            writeln!(file, "\n{}", "-".repeat(60))?;
            writeln!(file, "MANUAL REVIEW NEEDED")?;
            writeln!(file, "{}", "-".repeat(60))?;
            writeln!(
                file,
                "These require manual inspection (similar quality or missing data).\n"
            )?;

            for (fp_hash, tracks) in &manual_review {
                let first_track = tracks[0];
                let track_info =
                    if let (Some(artist), Some(title)) = (&first_track.artist, &first_track.title) {
                        format!("{} - {}", artist, title)
                    } else {
                        "Unknown track".to_string()
                    };

                writeln!(file, "Track: {}", track_info)?;
                writeln!(
                    file,
                    "Fingerprint: {}...",
                    &fp_hash.chars().take(40).collect::<String>()
                )?;
                writeln!(file, "Instances: {}", tracks.len())?;

                for track in *tracks {
                    let bitrate_str = track
                        .bitrate_kbps
                        .map(|br| format!("{}kbps", br))
                        .unwrap_or_else(|| "unknown".to_string());
                    let artist_str = track.artist.as_deref().unwrap_or("Unknown");
                    let title_str = track.title.as_deref().unwrap_or("Unknown");
                    writeln!(
                        file,
                        "  [{}] [{}] {} - {} - {}",
                        track.source,
                        track.file_type.to_uppercase(),
                        bitrate_str,
                        artist_str,
                        title_str
                    )?;
                    writeln!(file, "      {}", track.path)?;
                }
                writeln!(file)?;
            }
        }
    }

    Ok(format!(
        "Duplicate report generated: {}\n  Metadata duplicates: {}\n  Fingerprint duplicates: {} ({} auto-resolvable, {} manual review)",
        output_path.display(),
        metadata_duplicates.len(),
        fingerprint_duplicates.len(),
        auto_resolvable.len(),
        manual_review.len()
    ))
}

/// Group tracks by their deployment path
/// This ensures we only flag as duplicates tracks that would deploy to the SAME location
fn group_by_deployment_path<'a>(
    tracks: Vec<&'a Track>,
    _config: &config::Config,
) -> HashMap<String, Vec<&'a Track>> {
    let mut groups: HashMap<String, Vec<&'a Track>> = HashMap::new();

    for track in tracks {
        let deploy_path = super::deploy::compute_deployment_path(track);
        let key = deploy_path.to_string_lossy().to_string();
        groups.entry(key).or_default().push(track);
    }

    groups
}

// ============================================================================
// Health Summary Functions (reading from health_issues table)
// ============================================================================

use crate::corpus::db::{HealthIssue, HealthIssueType, HealthIssueSeverity, HealthSummary};

/// Get the current health summary from the database.
/// This reads from the health_issues table for fast cached results.
pub fn get_health_summary() -> Result<HealthSummary> {
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;
    db.get_health_summary()
}

/// Format a health summary as a brief string for display in the UI.
pub fn format_health_summary_brief(summary: &HealthSummary) -> String {
    let total_issues = summary.fingerprint_duplicates
        + summary.metadata_duplicates
        + summary.canonicalization_issues
        + summary.missing_tag_issues
        + summary.quality_variants;

    if total_issues == 0 {
        return "Corpus is healthy - no issues detected".to_string();
    }

    let mut parts = Vec::new();

    if summary.fingerprint_duplicates > 0 {
        parts.push(format!("{} fingerprint dups", summary.fingerprint_duplicates));
    }
    if summary.metadata_duplicates > 0 {
        parts.push(format!("{} metadata dups", summary.metadata_duplicates));
    }
    if summary.canonicalization_issues > 0 {
        parts.push(format!("{} canon issues", summary.canonicalization_issues));
    }
    if summary.missing_tag_issues > 0 {
        parts.push(format!("{} missing tags", summary.missing_tag_issues));
    }
    if summary.quality_variants > 0 {
        parts.push(format!("{} quality variants", summary.quality_variants));
    }
    if summary.known_variants > 0 {
        parts.push(format!("{} known variants", summary.known_variants));
    }

    let resolvable_note = if summary.auto_resolvable > 0 {
        format!(" ({} auto-resolvable)", summary.auto_resolvable)
    } else {
        String::new()
    };

    format!("{}{}", parts.join(", "), resolvable_note)
}

/// Generate a detailed health report file from the health_issues table.
pub fn generate_health_report(output_path: &Path) -> Result<String> {
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;

    // Get health summary
    let summary = db.get_health_summary()?;

    // Get unresolved issues by type
    let fingerprint_issues = db.get_unresolved_health_issues(Some(HealthIssueType::FingerprintDuplicate))?;
    let metadata_issues = db.get_unresolved_health_issues(Some(HealthIssueType::MetadataDuplicate))?;

    // Write report
    let mut file = File::create(output_path)?;

    writeln!(file, "Music Library Assistant - Corpus Health Report")?;
    writeln!(file, "==============================================")?;
    writeln!(file, "Generated: {}\n", chrono::Local::now().format("%Y-%m-%d %H:%M:%S"))?;

    // Summary section
    writeln!(file, "SUMMARY")?;
    writeln!(file, "-------")?;
    writeln!(file, "Fingerprint duplicates: {}", summary.fingerprint_duplicates)?;
    writeln!(file, "Metadata duplicates: {}", summary.metadata_duplicates)?;
    writeln!(file, "Canonicalization issues: {}", summary.canonicalization_issues)?;
    writeln!(file, "Missing tag issues: {}", summary.missing_tag_issues)?;
    writeln!(file, "Quality variants: {}", summary.quality_variants)?;
    writeln!(file, "Known variants (re-releases): {}", summary.known_variants)?;
    writeln!(file)?;
    writeln!(file, "Auto-resolvable: {}", summary.auto_resolvable)?;
    writeln!(file, "Manual review needed: {}\n", summary.manual_review)?;

    // Fingerprint duplicates section
    if !fingerprint_issues.is_empty() {
        writeln!(file, "\n{}", "=".repeat(80))?;
        writeln!(file, "FINGERPRINT DUPLICATES")?;
        writeln!(file, "{}\n", "=".repeat(80))?;

        for issue in &fingerprint_issues {
            write_health_issue(&mut file, &db, issue)?;
        }
    }

    // Metadata duplicates section
    if !metadata_issues.is_empty() {
        writeln!(file, "\n{}", "=".repeat(80))?;
        writeln!(file, "METADATA DUPLICATES")?;
        writeln!(file, "{}\n", "=".repeat(80))?;

        for issue in &metadata_issues {
            write_health_issue(&mut file, &db, issue)?;
        }
    }

    Ok(format!(
        "Health report generated: {}\n  {} fingerprint issues, {} metadata issues",
        output_path.display(),
        fingerprint_issues.len(),
        metadata_issues.len()
    ))
}

/// Write a single health issue to the report file.
fn write_health_issue(file: &mut File, db: &Database, issue: &HealthIssue) -> Result<()> {
    let severity_str = match issue.severity {
        HealthIssueSeverity::AutoResolvable => "[AUTO]",
        HealthIssueSeverity::ManualReview => "[MANUAL]",
        HealthIssueSeverity::Informational => "[INFO]",
    };

    writeln!(file, "Issue: {} {}", severity_str, truncate_key(&issue.issue_key, 60))?;

    // Get tracks for this issue
    if let Some(issue_id) = issue.id {
        let tracks = db.get_health_issue_tracks(issue_id)?;
        writeln!(file, "Tracks: {}", tracks.len())?;

        for (track, role) in &tracks {
            let bitrate_str = track
                .bitrate_kbps
                .map(|br| format!("{}kbps", br))
                .unwrap_or_else(|| "?".to_string());
            let role_str = role.as_str();
            writeln!(
                file,
                "  [{}] [{}] [{}] {} - {}",
                role_str,
                track.file_type.to_uppercase(),
                bitrate_str,
                track.artist.as_deref().unwrap_or("Unknown"),
                track.title.as_deref().unwrap_or("Unknown")
            )?;
            writeln!(file, "      {}", track.path)?;
        }
    }

    writeln!(file)?;
    Ok(())
}

/// Generate a report of known variants (legitimate re-releases, remixes, etc.)
/// This is separate from the duplicate report to keep actionable items distinct.
pub fn generate_known_variants_report(output_path: &Path) -> Result<String> {
    let db_path = config::get_db_path()?;
    let db = Database::open(&db_path)?;

    // Get all known variants
    let variants = db.get_all_known_variants()?;

    if variants.is_empty() {
        let mut file = File::create(output_path)?;
        writeln!(file, "Music Library Assistant - Known Variants Report")?;
        writeln!(file, "===============================================\n")?;
        writeln!(file, "No known variants have been marked.")?;
        writeln!(file, "\nKnown variants are tracks that share the same fingerprint but are")?;
        writeln!(file, "legitimate different versions (re-releases, remasters, remixes, live).")?;
        return Ok(format!("Known variants report generated: {} (0 variants)", output_path.display()));
    }

    let mut file = File::create(output_path)?;
    writeln!(file, "Music Library Assistant - Known Variants Report")?;
    writeln!(file, "===============================================\n")?;
    writeln!(file, "Total known variants: {}\n", variants.len())?;
    writeln!(file, "These are tracks that share fingerprints but are legitimate different versions.")?;
    writeln!(file, "They have been marked as known variants and are excluded from duplicate detection.\n")?;

    // Group by variant type
    use std::collections::HashMap;
    let mut by_type: HashMap<String, Vec<_>> = HashMap::new();
    for variant in &variants {
        let type_str = variant.variant_type.as_str().to_string();
        by_type.entry(type_str).or_default().push(variant);
    }

    for (variant_type, type_variants) in &by_type {
        writeln!(file, "\n{}", "=".repeat(60))?;
        writeln!(file, "{} ({})", variant_type.to_uppercase(), type_variants.len())?;
        writeln!(file, "{}\n", "=".repeat(60))?;

        for variant in type_variants {
            // Get track info for canonical
            let canonical_info = variant.canonical_track_id
                .and_then(|id| db.get_track_by_id(id).ok().flatten())
                .map(|t| format!(
                    "{} - {} ({})",
                    t.artist.as_deref().unwrap_or("Unknown"),
                    t.title.as_deref().unwrap_or("Unknown"),
                    t.path
                ))
                .unwrap_or_else(|| format!("Fingerprint: {}...", &variant.canonical_fingerprint.chars().take(40).collect::<String>()));

            // Get track info for variant (if different fingerprint)
            let variant_info = variant.variant_track_id
                .and_then(|id| db.get_track_by_id(id).ok().flatten())
                .map(|t| format!(
                    "{} - {} ({})",
                    t.artist.as_deref().unwrap_or("Unknown"),
                    t.title.as_deref().unwrap_or("Unknown"),
                    t.path
                ));

            writeln!(file, "Canonical: {}", canonical_info)?;
            if let Some(vi) = variant_info {
                writeln!(file, "  Variant: {}", vi)?;
            }
            if let Some(ref notes) = variant.notes {
                writeln!(file, "  Notes: {}", notes)?;
            }
            if let Some(ref marked_at) = variant.marked_at {
                writeln!(file, "  Marked: {}", marked_at)?;
            }
            writeln!(file)?;
        }
    }

    Ok(format!(
        "Known variants report generated: {}\n  {} variants across {} types",
        output_path.display(),
        variants.len(),
        by_type.len()
    ))
}

/// Truncate a string key for display, UTF-8 safe.
fn truncate_key(key: &str, max_chars: usize) -> String {
    let char_count = key.chars().count();
    if char_count <= max_chars {
        key.to_string()
    } else {
        format!("{}...", key.chars().take(max_chars - 3).collect::<String>())
    }
}
