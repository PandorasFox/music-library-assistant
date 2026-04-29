//! Tag generation from MusicBrainz cached data.
//!
//! Pure functions: MB data in, TagOps out. No DB access.
//! Translates approved release packing results into tag edit operations
//! using configurable credit routing and locale preferences.

use std::collections::{HashMap, HashSet};

use crate::config::{CreditRoutingConfig, MbTagNameConfig};
use crate::external::musicbrainz::{
    resolve_artist_name, join_artist_credits_localized,
    MbArtist, MbArtistCredit, MbArtistRef, MbRecording, MbRelease,
};
use crate::mutations::TagOp;

/// MusicBrainz MBID for the special "Various Artists" artist. Used to detect
/// genuine VA-credited compilations (where MB itself credits the release to VA)
/// vs. multi-artist releases like "Deadmau5 vs Meleefresh" which keep the
/// joinphrase-rendered display name.
pub const MB_VARIOUS_ARTISTS_ID: &str = "89ad4ac3-39f7-470e-963a-56509c546377";

// ============================================================================
// Input Types
// ============================================================================

/// All data needed to generate tags for a single inode.
pub struct MbTagInput {
    pub inode: i64,
    pub recording_id: String,
    pub release_id: String,
    /// Track title from the release tracklist (may differ from recording title).
    pub track_title: String,
    pub track_position: u32,
    pub medium_position: u32,
    /// Total number of media in the release (emit DISCNUMBER only if > 1).
    pub total_media: u32,
    /// Current tags on this inode: (UPPERCASE tag_name, value) pairs.
    pub current_tags: Vec<(String, String)>,
}

// ============================================================================
// Core Tag Generation
// ============================================================================

/// Generate TagOps for one inode from MB cached data.
///
/// Computes desired tag values from the release/recording/artist data,
/// diffs against current tags, and produces the minimal set of TagOps.
pub fn generate_tag_ops(
    input: &MbTagInput,
    recording: &MbRecording,
    release: &MbRelease,
    artists: &HashMap<String, MbArtist>,
    locales: &[String],
    routing: &CreditRoutingConfig,
    tag_names: &MbTagNameConfig,
) -> Vec<TagOp> {
    let mut desired: Vec<(String, String)> = Vec::new();

    // TITLE: track title + optional feat suffix
    let feat_suffix = generate_feat_suffix(recording, artists, locales, routing);
    let title = if let Some(suffix) = feat_suffix {
        format!("{} ({})", input.track_title, suffix)
    } else {
        input.track_title.clone()
    };
    desired.push(("TITLE".to_string(), title));

    // ARTIST / ARTISTS: Navidrome singular/plural convention.
    // Singular = display string (joinphrase-based), plural = individual decomposed values.
    // Only emit both forms when there are actually multiple artists.
    let individual_artists = extract_individual_artists(
        recording, artists, locales, routing,
    );
    let individual_artist_ids = extract_individual_artist_ids(recording, routing);
    if individual_artists.len() > 1 {
        let recording_artists: Vec<(String, Option<MbArtist>)> = recording
            .artist_credit
            .iter()
            .map(|c| {
                let artist = artists.get(&c.artist.id).cloned();
                (c.artist.id.clone(), artist)
            })
            .collect();
        let artist_display =
            join_artist_credits_localized(&recording.artist_credit, &recording_artists, locales);
        desired.push(("ARTIST".to_string(), artist_display));
        for artist_name in &individual_artists {
            desired.push(("ARTISTS".to_string(), artist_name.clone()));
        }
    } else {
        for artist_name in &individual_artists {
            desired.push(("ARTIST".to_string(), artist_name.clone()));
        }
    }
    // MUSICBRAINZ_ARTISTID (multi-value): one per individual recording artist.
    // Mirrors the order/dedup of ARTIST(S) above so Navidrome can correlate
    // by MBID even when display names drift across releases/aliases.
    for artist_id in &individual_artist_ids {
        desired.push((tag_names.artist_id.clone(), artist_id.clone()));
    }

    // ALBUM: release title
    desired.push(("ALBUM".to_string(), release.title.clone()));

    // Various Artists detection: MB explicitly credits the release to the
    // special "Various Artists" artist. Only then do we collapse ALBUMARTIST
    // to "Various Artists" — multi-artist releases like "A vs B" keep their
    // joinphrase-rendered display name.
    let is_va_release = release
        .artist_credit
        .iter()
        .any(|c| c.artist.id == MB_VARIOUS_ARTISTS_ID);

    // ALBUMARTIST / ALBUMARTISTS: Navidrome singular/plural convention.
    // Singular = display string, plural = individual release-level credits.
    let release_artists: Vec<(String, Option<MbArtist>)> = release
        .artist_credit
        .iter()
        .map(|c| {
            let artist = artists.get(&c.artist.id).cloned();
            (c.artist.id.clone(), artist)
        })
        .collect();
    let album_artist_display = if is_va_release {
        "Various Artists".to_string()
    } else {
        join_artist_credits_localized(&release.artist_credit, &release_artists, locales)
    };
    desired.push(("ALBUMARTIST".to_string(), album_artist_display));
    // Only emit ALBUMARTISTS for non-VA multi-credit releases. VA releases
    // collapse to a single albumartist; the per-track ARTISTS still carry the
    // real per-track credits for Navidrome to surface.
    if !is_va_release && release.artist_credit.len() > 1 {
        for credit in &release.artist_credit {
            let resolved = resolve_artist_name(
                credit,
                artists.get(&credit.artist.id),
                locales,
            );
            desired.push(("ALBUMARTISTS".to_string(), resolved));
        }
    }
    // MUSICBRAINZ_ALBUMARTISTID (multi-value): one per release credit.
    // For VA releases this is just the VA MBID. This is what Navidrome uses
    // as the canonical join key when display strings vary between scrapes.
    for credit in &release.artist_credit {
        desired.push((
            tag_names.albumartist_id.clone(),
            credit.artist.id.clone(),
        ));
    }

    // COMPILATION flag: set when MB credits the release to "Various Artists"
    // OR the release-group's secondary types include "Compilation". The
    // release-group check requires `inc=release-groups` on the fetch; older
    // cached releases without this data silently skip (no harm — the VA
    // check above catches the most common case).
    let is_compilation_rg = release
        .release_group
        .as_ref()
        .map(|rg| {
            rg.secondary_types
                .iter()
                .any(|t| t.eq_ignore_ascii_case("Compilation"))
        })
        .unwrap_or(false);
    if is_va_release || is_compilation_rg {
        desired.push(("COMPILATION".to_string(), "1".to_string()));
    }

    // TRACKNUMBER
    desired.push((
        "TRACKNUMBER".to_string(),
        input.track_position.to_string(),
    ));

    // DISCNUMBER (only for multi-disc releases)
    if input.total_media > 1 {
        desired.push((
            "DISCNUMBER".to_string(),
            input.medium_position.to_string(),
        ));
    }

    // MB IDs (using configured tag names)
    desired.push((
        tag_names.release.clone(),
        input.release_id.clone(),
    ));
    desired.push((
        tag_names.recording.clone(),
        input.recording_id.clone(),
    ));
    // Track ID: look up from release media by position
    let track_id = release
        .media
        .iter()
        .find(|m| m.position == input.medium_position)
        .and_then(|m| m.tracks.iter().find(|t| t.position == input.track_position))
        .map(|t| t.id.as_str())
        .unwrap_or("");
    if !track_id.is_empty() {
        desired.push((
            tag_names.track.clone(),
            track_id.to_string(),
        ));
    }

    // Picard-compatible aliases (for Navidrome and other Picard-aware consumers).
    // Only emitted when enabled, and skipped if the configured name already IS
    // the Picard name (no duplicate tags).
    if tag_names.picard_compat {
        if tag_names.release != MbTagNameConfig::PICARD_RELEASE {
            desired.push((
                MbTagNameConfig::PICARD_RELEASE.to_string(),
                input.release_id.clone(),
            ));
        }
        if tag_names.recording != MbTagNameConfig::PICARD_RECORDING {
            desired.push((
                MbTagNameConfig::PICARD_RECORDING.to_string(),
                input.recording_id.clone(),
            ));
        }
        if !track_id.is_empty() && tag_names.track != MbTagNameConfig::PICARD_TRACK {
            desired.push((
                MbTagNameConfig::PICARD_TRACK.to_string(),
                track_id.to_string(),
            ));
        }
    }

    compute_tag_diff(input.inode, &desired, &input.current_tags)
}

// ============================================================================
// Artist Decomposition
// ============================================================================

/// Extract individual artist names from recording credits + relations.
///
/// Returns deduplicated list of locale-resolved artist names. Sources:
/// 1. Recording artist_credit entries (each credit → one artist)
/// 2. Recording relations where routing routes to `artist`
///    Deduplicated by artist MBID.
pub fn extract_individual_artists(
    recording: &MbRecording,
    artists: &HashMap<String, MbArtist>,
    locales: &[String],
    routing: &CreditRoutingConfig,
) -> Vec<String> {
    let mut seen_ids = HashSet::new();
    let mut result = Vec::new();

    // 1. Recording artist credits — always included as artists
    for credit in &recording.artist_credit {
        if seen_ids.insert(credit.artist.id.clone()) {
            let resolved = resolve_artist_name(
                credit,
                artists.get(&credit.artist.id),
                locales,
            );
            result.push(resolved);
        }
    }

    // 2. Recording relations routed to artist
    for relation in &recording.relations {
        let route = routing.route_for(&relation.type_);
        if !route.artist {
            continue;
        }
        if let Some(ref rel_artist) = relation.artist {
            if seen_ids.insert(rel_artist.id.clone()) {
                // Build a synthetic MbArtistCredit for resolve_artist_name
                let credit = MbArtistCredit {
                    name: rel_artist.name.clone(),
                    joinphrase: String::new(),
                    artist: MbArtistRef {
                        id: rel_artist.id.clone(),
                        name: rel_artist.name.clone(),
                        sort_name: rel_artist.sort_name.clone(),
                    },
                };
                let resolved = resolve_artist_name(
                    &credit,
                    artists.get(&rel_artist.id),
                    locales,
                );
                result.push(resolved);
            }
        }
    }

    result
}

/// Extract individual artist MBIDs from recording credits + relations, mirroring
/// the iteration order of `extract_individual_artists`. Used to emit
/// `MUSICBRAINZ_ARTISTID` aligned with `ARTIST(S)` values.
pub fn extract_individual_artist_ids(
    recording: &MbRecording,
    routing: &CreditRoutingConfig,
) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut result = Vec::new();
    for credit in &recording.artist_credit {
        if seen.insert(credit.artist.id.clone()) {
            result.push(credit.artist.id.clone());
        }
    }
    for relation in &recording.relations {
        if !routing.route_for(&relation.type_).artist {
            continue;
        }
        if let Some(ref a) = relation.artist {
            if seen.insert(a.id.clone()) {
                result.push(a.id.clone());
            }
        }
    }
    result
}

// ============================================================================
// Featured Artist Suffix
// ============================================================================

/// Generate a "feat. A, B & C" suffix from recording relations routed to title.
///
/// Only includes artists routed to `title` who are NOT already in the recording's
/// primary artist credits (to avoid "Artist feat. Artist").
pub fn generate_feat_suffix(
    recording: &MbRecording,
    artists: &HashMap<String, MbArtist>,
    locales: &[String],
    routing: &CreditRoutingConfig,
) -> Option<String> {
    // Collect primary credit artist IDs to exclude from feat suffix
    let primary_ids: HashSet<&str> = recording
        .artist_credit
        .iter()
        .map(|c| c.artist.id.as_str())
        .collect();

    let mut feat_names = Vec::new();

    for relation in &recording.relations {
        let route = routing.route_for(&relation.type_);
        if !route.title {
            continue;
        }
        if let Some(ref rel_artist) = relation.artist {
            // Skip artists already in primary credits
            if primary_ids.contains(rel_artist.id.as_str()) {
                continue;
            }
            let credit = MbArtistCredit {
                name: rel_artist.name.clone(),
                joinphrase: String::new(),
                artist: MbArtistRef {
                    id: rel_artist.id.clone(),
                    name: rel_artist.name.clone(),
                    sort_name: rel_artist.sort_name.clone(),
                },
            };
            let resolved = resolve_artist_name(
                &credit,
                artists.get(&rel_artist.id),
                locales,
            );
            feat_names.push(resolved);
        }
    }

    if feat_names.is_empty() {
        return None;
    }

    // Deduplicate while preserving order
    let mut seen = HashSet::new();
    feat_names.retain(|n| seen.insert(n.clone()));

    // Cap the number of feat credits if configured
    if let Some(max) = routing.max_feat_credits {
        feat_names.truncate(max as usize);
    }

    // Join: "A, B & C"
    let joined = if feat_names.len() == 1 {
        feat_names[0].clone()
    } else {
        let (last, rest) = feat_names.split_last().unwrap();
        format!("{} & {}", rest.join(", "), last)
    };

    // Apply feat_format template
    let formatted = routing.feat_format.replace("{artists}", &joined);
    Some(formatted)
}

// ============================================================================
// Tag Diff
// ============================================================================

/// Compute the minimal set of TagOps to transform current tags into desired tags.
///
/// Handles multi-value tags cleanly: for a given tag name, drops values present
/// in current but absent from desired, adds values present in desired but absent
/// from current, leaves shared values untouched.
///
/// When the value count is unchanged (same number of values being removed and
/// added), pairs them as `replace_tag` ops instead of separate drop+add.
fn compute_tag_diff(
    inode: i64,
    desired: &[(String, String)],
    current: &[(String, String)],
) -> Vec<TagOp> {
    let mut ops = Vec::new();

    // Group desired and current by tag name
    let desired_by_name = group_by_name(desired);
    let current_by_name = group_by_name(current);

    // All tag names we care about (only names that appear in desired)
    for (&tag_name, desired_values) in &desired_by_name {
        let current_values: HashSet<&str> = current_by_name
            .get(tag_name)
            .map(|v| v.iter().copied().collect())
            .unwrap_or_default();
        let desired_set: HashSet<&str> = desired_values.iter().copied().collect();

        let to_drop: Vec<&str> = current_values
            .iter()
            .filter(|v| !desired_set.contains(*v))
            .copied()
            .collect();
        let to_add: Vec<&str> = desired_set
            .iter()
            .filter(|v| !current_values.contains(*v))
            .copied()
            .collect();

        if to_drop.len() == to_add.len() {
            // Same count changing: pair as replacements.
            for (old, new) in to_drop.iter().zip(to_add.iter()) {
                ops.push(TagOp::replace_tag(inode, tag_name, *old, *new));
            }
        } else {
            // Count differs: explicit drops and adds.
            for val in &to_drop {
                ops.push(TagOp::drop_tag(inode, tag_name, *val));
            }
            for val in &to_add {
                ops.push(TagOp::add_tag(inode, tag_name, *val));
            }
        }
    }

    ops
}

/// Group (tag_name, value) pairs by tag name.
fn group_by_name<'a>(tags: &'a [(String, String)]) -> HashMap<&'a str, Vec<&'a str>> {
    let mut map: HashMap<&'a str, Vec<&'a str>> = HashMap::new();
    for (name, value) in tags {
        map.entry(name.as_str()).or_default().push(value.as_str());
    }
    map
}
