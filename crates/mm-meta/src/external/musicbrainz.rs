//! MusicBrainz typed response structures and locale-aware name resolution.
//!
//! Pure data types parsed from cached JSON. No HTTP client or DB access.
//!
//! API docs: https://musicbrainz.org/doc/MusicBrainz_API

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

// ============================================================================
// Typed Response Structs (parsed on demand from cached JSON)
// ============================================================================

/// A MusicBrainz recording with credits and relations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbRecording {
    pub id: String,
    pub title: String,
    /// Duration in milliseconds (MB calls this "length").
    #[serde(default)]
    pub length: Option<i64>,
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<MbArtistCredit>,
    #[serde(default)]
    pub relations: Vec<MbRelation>,
    #[serde(default)]
    pub releases: Vec<MbReleaseRef>,
}

/// A release reference within a recording response (minimal).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbReleaseRef {
    pub id: String,
    pub title: Option<String>,
    #[serde(default, rename = "release-group")]
    pub release_group: Option<MbReleaseGroupRef>,
}

/// A release group reference (nested in release).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbReleaseGroupRef {
    pub id: String,
}

/// An artist credit entry on a recording or release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbArtistCredit {
    /// Credited name on this recording (may differ from canonical).
    pub name: String,
    /// Join phrase between this artist and the next (e.g., " feat. ", " & ").
    #[serde(default)]
    pub joinphrase: String,
    /// Nested artist object with canonical info.
    pub artist: MbArtistRef,
}

/// Minimal artist reference (nested in credits/relations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbArtistRef {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "sort-name")]
    pub sort_name: String,
}

/// A full MusicBrainz artist with aliases (from artist endpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbArtist {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "sort-name")]
    pub sort_name: String,
    #[serde(default)]
    pub aliases: Vec<MbAlias>,
}

/// An artist alias (locale-specific name variant).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbAlias {
    pub name: String,
    pub locale: Option<String>,
    /// "primary" if this is the primary alias for that locale.
    pub primary: Option<String>,
    /// "Artist name", "Legal name", "Search hint", etc.
    #[serde(rename = "type")]
    pub type_: Option<String>,
}

/// A relation on a recording or release.
///
/// MB serializes many flavors into one shape; the resource pointed at varies
/// by `type_`. Artist-recording relations populate `artist`; URL relations
/// (from `inc=url-rels`) populate `url`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbRelation {
    /// Relation type: "vocal", "producer", "remixer", "composer", "discogs",
    /// "wikidata", "allmusic", etc. For URL relations the type names the
    /// destination site (e.g. `"discogs"`).
    #[serde(rename = "type")]
    pub type_: String,
    /// "backward" = artist→recording direction. Direction is meaningless for
    /// URL relations.
    pub direction: Option<String>,
    /// Qualifiers: ["lead vocals"], ["guitar", "bass"], etc.
    #[serde(default)]
    pub attributes: Vec<String>,
    /// Related artist (present for artist-recording relations).
    pub artist: Option<MbArtistRef>,
    /// URL target (present for `inc=url-rels` entries).
    pub url: Option<MbUrlTarget>,
}

/// Minimal URL target carried by URL-relation entries.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbUrlTarget {
    /// The URL itself, e.g. `https://www.discogs.com/release/12345`.
    pub resource: String,
}

/// A full MusicBrainz release with artist credits and tracklist (from release endpoint).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbRelease {
    pub id: String,
    pub title: String,
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<MbArtistCredit>,
    /// Media (discs/sides) with track listings. Empty for old cached JSON
    /// that was fetched without `inc=recordings+media`.
    #[serde(default)]
    pub media: Vec<MbMedium>,
    /// Release group classification (primary type, secondary types like "Compilation").
    /// Present only when the release was fetched with `inc=release-groups`; older
    /// cached JSON has this as `None` and compilation detection silently skips.
    #[serde(default, rename = "release-group")]
    pub release_group: Option<MbReleaseGroup>,
    /// URL relations (from `inc=url-rels`), notably the linked Discogs release.
    /// Empty for old cached JSON fetched without `url-rels`.
    #[serde(default)]
    pub relations: Vec<MbRelation>,
}

/// Release-group classification (from release lookup with `inc=release-groups`).
///
/// Used for compilation detection: a release is a compilation if `secondary_types`
/// contains "Compilation".
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbReleaseGroup {
    pub id: String,
    #[serde(default, rename = "primary-type")]
    pub primary_type: Option<String>,
    #[serde(default, rename = "secondary-types")]
    pub secondary_types: Vec<String>,
}

/// A medium within a release (CD, vinyl side, digital media, etc.).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbMedium {
    /// Medium position (1-indexed: disc 1, disc 2, etc.).
    pub position: u32,
    /// Medium format ("CD", "Digital Media", "12\" Vinyl", etc.).
    pub format: Option<String>,
    /// Track list for this medium.
    #[serde(default)]
    pub tracks: Vec<MbTrack>,
    /// Data tracks (MB API uses this instead of `tracks` for CD-R, enhanced CD,
    /// and other data-bearing media). Same schema as `tracks`.
    #[serde(default, rename = "data-tracks")]
    pub data_tracks: Vec<MbTrack>,
}

/// A track within a medium (position + recording reference).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbTrack {
    /// Track MBID (the track-on-release identifier, distinct from the recording MBID).
    #[serde(default)]
    pub id: String,
    /// Track position within the medium (1-indexed).
    pub position: u32,
    /// Track number as printed (e.g., "A1", "3", etc.).
    pub number: String,
    /// Track title (may differ from recording title for compilations).
    pub title: String,
    /// Duration in milliseconds (track-level, may differ from recording).
    #[serde(default)]
    pub length: Option<i64>,
    /// The recording this track points to.
    pub recording: MbTrackRecording,
}

/// Minimal recording reference within a track.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbTrackRecording {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub length: Option<i64>,
}

// ============================================================================
// Batch Cache Bundle (struct only — load() stays in mm)
// ============================================================================

/// Batch-loaded MB data from the database cache.
///
/// Provides parsed releases, recordings, and artists in HashMaps keyed by MBID.
/// Artist loading is transitive: artists referenced by release credits and
/// recording credits/relations are automatically chased.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MbCacheBundle {
    pub releases: HashMap<String, MbRelease>,
    pub recordings: HashMap<String, MbRecording>,
    pub artists: HashMap<String, MbArtist>,
}

// ============================================================================
// Locale-Aware Artist Name Resolution
// ============================================================================

/// Resolve a single artist credit's display name using locale preferences.
///
/// Walks `locales` in order, searching the artist's aliases for a matching locale.
/// Prefers aliases marked `primary == "primary"` and `type_ == "Artist name"`.
/// Falls back to the credit's `name` field (as-credited / native script).
pub fn resolve_artist_name(
    credit: &MbArtistCredit,
    artist: Option<&MbArtist>,
    locales: &[String],
) -> String {
    let Some(artist) = artist else {
        return credit.name.clone();
    };

    for locale in locales {
        let locale_lower = locale.to_lowercase();

        // Find all aliases matching this locale.
        let mut matches: Vec<&MbAlias> = artist
            .aliases
            .iter()
            .filter(|a| {
                a.locale
                    .as_ref()
                    .is_some_and(|l| l.to_lowercase() == locale_lower)
            })
            .collect();

        if matches.is_empty() {
            continue;
        }

        // Sort: primary "Artist name" > primary other > non-primary "Artist name" > rest
        matches.sort_by(|a, b| {
            let score = |alias: &MbAlias| -> u8 {
                let is_primary = alias.primary.as_deref() == Some("primary");
                let is_artist_name = alias.type_.as_deref() == Some("Artist name");
                match (is_primary, is_artist_name) {
                    (true, true) => 3,
                    (true, false) => 2,
                    (false, true) => 1,
                    (false, false) => 0,
                }
            };
            score(b).cmp(&score(a))
        });

        return matches[0].name.clone();
    }

    credit.name.clone()
}

/// Join artist credits into a display string using locale-aware name resolution.
///
/// Each credit is resolved against its corresponding artist data (if cached),
/// then joined with the credit's joinphrase.
pub fn join_artist_credits_localized(
    credits: &[MbArtistCredit],
    artists: &[(String, Option<MbArtist>)],
    locales: &[String],
) -> String {
    // Build a lookup from artist MBID → &MbArtist.
    let artist_map: HashMap<&str, &MbArtist> = artists
        .iter()
        .filter_map(|(id, opt)| opt.as_ref().map(|a| (id.as_str(), a)))
        .collect();

    let mut result = String::new();
    for (i, credit) in credits.iter().enumerate() {
        let resolved = resolve_artist_name(
            credit,
            artist_map.get(credit.artist.id.as_str()).copied(),
            locales,
        );
        result.push_str(&resolved);
        if i < credits.len() - 1 {
            if credit.joinphrase.is_empty() {
                result.push_str(", ");
            } else {
                result.push_str(&credit.joinphrase);
            }
        }
    }
    result
}

// ============================================================================
// Parse Helpers
// ============================================================================

/// Parse cached recording JSON into typed struct.
pub fn parse_recording(raw_json: &[u8]) -> Result<MbRecording> {
    serde_json::from_slice(raw_json).context("Failed to parse cached MusicBrainz recording JSON")
}

/// Parse cached artist JSON into typed struct.
pub fn parse_artist(raw_json: &[u8]) -> Result<MbArtist> {
    serde_json::from_slice(raw_json).context("Failed to parse cached MusicBrainz artist JSON")
}

/// Parse cached release JSON into typed struct.
///
/// Merges `data-tracks` into `tracks` for each medium — the MB API separates
/// them for CD-R / enhanced CD / data media, but we treat all tracks uniformly.
pub fn parse_release(raw_json: &[u8]) -> Result<MbRelease> {
    let mut release: MbRelease =
        serde_json::from_slice(raw_json).context("Failed to parse cached MusicBrainz release JSON")?;
    for medium in &mut release.media {
        if !medium.data_tracks.is_empty() {
            medium.tracks.append(&mut medium.data_tracks);
            medium.tracks.sort_by_key(|t| t.position);
        }
    }
    Ok(release)
}

/// Extract the linked Discogs release id from a relations list, if present.
///
/// MB serializes URL relations to Discogs as type `"discogs"` with the
/// destination URL on `url.resource`. We accept either form:
///   - `https://www.discogs.com/release/12345`
///   - `https://www.discogs.com/release/12345-Some-Title-Slug`
///   - `http://www.discogs.com/release/12345`
///
/// Returns `None` if no Discogs release relation exists or the URL doesn't
/// look like a `/release/<id>` path (we deliberately ignore `/master/<id>`,
/// `/artist/<id>`, etc., even when they appear under `type="discogs"`).
pub fn discogs_release_id_from_relations(relations: &[MbRelation]) -> Option<String> {
    for rel in relations {
        if rel.type_ != "discogs" {
            continue;
        }
        let Some(ref target) = rel.url else { continue };
        if let Some(id) = parse_discogs_release_id_from_url(&target.resource) {
            return Some(id);
        }
    }
    None
}

/// Extract the Discogs *master* release id from a relations list, if present.
/// Master releases group multiple regional pressings into one entry on Discogs.
/// Returns `None` when no master URL is in the relations.
pub fn discogs_master_id_from_relations(relations: &[MbRelation]) -> Option<String> {
    for rel in relations {
        if rel.type_ != "discogs" {
            continue;
        }
        let Some(ref target) = rel.url else { continue };
        if let Some(id) = parse_discogs_master_id_from_url(&target.resource) {
            return Some(id);
        }
    }
    None
}

/// Parse `https://www.discogs.com/release/12345[-slug]` → `Some("12345")`.
///
/// Public so callers (and tests) can validate a URL without constructing a
/// full `MbRelation` graph. Tolerant of trailing slugs and query strings,
/// strict about the `/release/` path segment.
pub fn parse_discogs_release_id_from_url(url: &str) -> Option<String> {
    parse_discogs_id_with_segment(url, "release")
}

/// Parse `https://www.discogs.com/master/12345[-slug]` → `Some("12345")`.
pub fn parse_discogs_master_id_from_url(url: &str) -> Option<String> {
    parse_discogs_id_with_segment(url, "master")
}

fn parse_discogs_id_with_segment(url: &str, segment: &str) -> Option<String> {
    // Look for `/<segment>/` anywhere in the URL, then take the next path
    // component and strip a trailing `-slug` if present. Keep the parser tight
    // — only accept all-digit ids so we don't accidentally return e.g. a
    // user/locale-prefixed path component.
    let needle = format!("/{segment}/");
    let after_segment = url.find(&needle).map(|i| &url[i + needle.len()..])?;
    let next_seg = after_segment
        .split(|c: char| c == '/' || c == '?' || c == '#')
        .next()?;
    // Strip slug: "12345-Some-Title" → "12345"
    let id = next_seg.split('-').next()?;
    if !id.is_empty() && id.chars().all(|c| c.is_ascii_digit()) {
        Some(id.to_string())
    } else {
        None
    }
}

#[cfg(test)]
mod discogs_link_tests {
    use super::*;

    #[test]
    fn parses_release_url_with_slug() {
        assert_eq!(
            parse_discogs_release_id_from_url(
                "https://www.discogs.com/release/12345-Some-Title-Slug"
            ),
            Some("12345".to_string()),
        );
    }

    #[test]
    fn parses_release_url_without_slug() {
        assert_eq!(
            parse_discogs_release_id_from_url("https://www.discogs.com/release/9876"),
            Some("9876".to_string()),
        );
    }

    #[test]
    fn parses_http_scheme() {
        assert_eq!(
            parse_discogs_release_id_from_url("http://www.discogs.com/release/42"),
            Some("42".to_string()),
        );
    }

    #[test]
    fn parses_locale_prefixed_url() {
        // Discogs often serves locale-prefixed paths like /ja/release/123.
        // Our parser finds `/release/` anywhere — locale prefix is fine.
        assert_eq!(
            parse_discogs_release_id_from_url("https://www.discogs.com/ja/release/123"),
            Some("123".to_string()),
        );
    }

    #[test]
    fn rejects_master_url() {
        assert!(parse_discogs_release_id_from_url(
            "https://www.discogs.com/master/12345"
        )
        .is_none());
    }

    #[test]
    fn rejects_artist_url() {
        assert!(parse_discogs_release_id_from_url(
            "https://www.discogs.com/artist/12345"
        )
        .is_none());
    }

    #[test]
    fn parses_master_url() {
        assert_eq!(
            parse_discogs_master_id_from_url("https://www.discogs.com/master/777-Master-Title"),
            Some("777".to_string()),
        );
    }

    #[test]
    fn rejects_non_numeric_id() {
        assert!(
            parse_discogs_release_id_from_url("https://www.discogs.com/release/abc").is_none()
        );
    }

    #[test]
    fn extracts_from_relations() {
        let rels = vec![
            MbRelation {
                type_: "wikidata".into(),
                direction: None,
                attributes: vec![],
                artist: None,
                url: Some(MbUrlTarget {
                    resource: "https://www.wikidata.org/wiki/Q123".into(),
                }),
            },
            MbRelation {
                type_: "discogs".into(),
                direction: None,
                attributes: vec![],
                artist: None,
                url: Some(MbUrlTarget {
                    resource: "https://www.discogs.com/release/42-Slug".into(),
                }),
            },
        ];
        assert_eq!(
            discogs_release_id_from_relations(&rels),
            Some("42".to_string()),
        );
    }

    #[test]
    fn empty_relations_returns_none() {
        assert!(discogs_release_id_from_relations(&[]).is_none());
    }
}
