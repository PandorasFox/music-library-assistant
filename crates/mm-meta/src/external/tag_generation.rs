//! Tag generation from MusicBrainz cached data.
//!
//! Pure functions: MB data in, TagOps out. No DB access.
//! Translates approved release packing results into tag edit operations
//! using configurable credit routing and locale preferences.

use std::collections::{HashMap, HashSet};

use crate::config::CreditRoutingConfig;
use crate::external::musicbrainz::{
    resolve_artist_name, join_artist_credits_localized,
    MbArtist, MbArtistCredit, MbArtistRef, MbRecording, MbRelease,
};
use crate::mutations::TagOp;

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

    // ARTIST: individual tags per artist (decomposed, not joined)
    let individual_artists = extract_individual_artists(
        recording, artists, locales, routing,
    );
    for artist_name in &individual_artists {
        desired.push(("ARTIST".to_string(), artist_name.clone()));
    }

    // ALBUM: release title
    desired.push(("ALBUM".to_string(), release.title.clone()));

    // ALBUMARTIST: joined release-level credits (the one legitimate joined field)
    let release_artists: Vec<(String, Option<MbArtist>)> = release
        .artist_credit
        .iter()
        .map(|c| {
            let artist = artists.get(&c.artist.id).cloned();
            (c.artist.id.clone(), artist)
        })
        .collect();
    let album_artist =
        join_artist_credits_localized(&release.artist_credit, &release_artists, locales);
    desired.push(("ALBUMARTIST".to_string(), album_artist));

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

    // MB IDs
    desired.push((
        "MUSICBRAINZ_RELEASEID".to_string(),
        input.release_id.clone(),
    ));
    desired.push((
        "MUSICBRAINZ_RECORDINGID".to_string(),
        input.recording_id.clone(),
    ));

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

        // Drop values in current but not in desired
        for val in &current_values {
            if !desired_set.contains(val) {
                ops.push(TagOp::drop_tag(inode, tag_name, *val));
            }
        }

        // Add values in desired but not in current
        for val in &desired_set {
            if !current_values.contains(val) {
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
