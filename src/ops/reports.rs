//! Report Module
//!
//! Part of MLA's toolkit: provides the "report" capability for analysis.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs::File;
use std::io::Write;
use std::path::Path;

use crate::config;
use crate::db::{Database, Track};

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
    let cfg = config::load_config()?;

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

    // Populate database with metadata duplicates for resolution UI
    // This allows the "Duplicate Resolution" menu to load and process these groups
    populate_metadata_duplicate_groups(&db, &track_groups, &cfg)?;

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

    // Filter to only duplicates
    let mut fingerprint_duplicates: Vec<_> = fingerprint_groups
        .iter()
        .filter(|(_, tracks)| tracks.len() > 1)
        .collect();

    fingerprint_duplicates.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

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
    writeln!(
        file,
        "Fingerprint-based duplicate groups: {}\n",
        fingerprint_duplicates.len()
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
        for (fp_hash, tracks) in &fingerprint_duplicates {
            // Show first track's metadata if available for context
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

    Ok(format!(
        "Duplicate report generated: {}\n  Metadata duplicates: {}\n  Fingerprint duplicates: {}",
        output_path.display(),
        metadata_duplicates.len(),
        fingerprint_duplicates.len()
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

/// Populate database with metadata duplicate groups
/// This function takes the metadata duplicates detected during report generation
/// and inserts them into the duplicate_groups and duplicate_group_members tables
/// so that the resolution UI can load and process them.
fn populate_metadata_duplicate_groups(
    db: &Database,
    track_groups: &HashMap<String, Vec<&Track>>,
    config: &config::Config,
) -> Result<()> {
    // 1. Clear stale pending metadata groups
    db.clear_pending_duplicate_groups("metadata")?;

    let mut groups_inserted = 0;
    let mut tracks_inserted = 0;

    // 2. For each metadata duplicate group
    for tracks in track_groups.values() {
        if tracks.len() < 2 {
            continue; // Skip non-duplicates
        }

        // 3. Group by deployment path
        // Only tracks that would deploy to the SAME path are true duplicates
        let by_deploy_path = group_by_deployment_path(tracks.clone(), config);

        // 4. Only insert groups where 2+ tracks deploy to SAME path
        for (deploy_path, matching_tracks) in by_deploy_path {
            if matching_tracks.len() < 2 {
                continue; // Skip if only 1 track would deploy to this path
            }

            // Insert the duplicate group
            let group_id = db.insert_duplicate_group("metadata", &deploy_path)?;
            groups_inserted += 1;

            // Insert all member tracks
            for track in matching_tracks {
                if let Some(track_id) = track.id {
                    db.insert_duplicate_group_member(group_id, track_id)?;
                    tracks_inserted += 1;
                }
            }
        }
    }

    // Log summary
    config::log_message(&format!(
        "Populated database: {} metadata duplicate groups with {} total tracks",
        groups_inserted, tracks_inserted
    ))?;

    Ok(())
}
