//! MusicBrainz API client and typed response structures.
//!
//! Fetches recording, artist, and release metadata from the MusicBrainz API.
//! Used by the MB fetch thread for background metadata enrichment after
//! AcoustID fingerprint matching identifies recording MBIDs.
//!
//! ## Response Types
//!
//! MusicBrainz has its own JSON API format. We define serde structs here
//! for on-demand parsing from cached raw JSON, not at fetch time.
//!
//! API docs: https://musicbrainz.org/doc/MusicBrainz_API

use std::collections::HashMap;

use anyhow::{Context, Result};
use serde::Deserialize;

// ============================================================================
// Client + Outcome Types
// ============================================================================

/// MusicBrainz API client.
pub struct MusicBrainzClient {
    agent: ureq::Agent,
    /// Base URL for the MusicBrainz WS/2 API (e.g. "https://musicbrainz.org/ws/2").
    base_url: String,
}

/// Outcome of a single MB API lookup.
pub enum MbLookupOutcome {
    /// Entity found — raw JSON bytes for cache storage.
    Found(Vec<u8>),
    /// 404 — entity does not exist.
    NotFound,
    /// 429 — rate limited.
    RateLimited,
    /// 503 — service unavailable (treat like rate limit for backoff).
    ServiceUnavailable,
}

impl MusicBrainzClient {
    pub fn new(base_url: &str) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("MusicMagic/0.1 (https://github.com/example/musicmagic)")
            .build();
        Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Returns the configured base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Fetch a recording by MBID with artist credits, artist relations, and work relations.
    pub fn fetch_recording(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!(
            "{}/recording/{}?inc=artist-credits+artist-rels+work-rels+releases&fmt=json",
            self.base_url, mbid
        );
        self.fetch_entity(&url)
    }

    /// Fetch an artist by MBID with aliases.
    pub fn fetch_artist(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!("{}/artist/{}?inc=aliases&fmt=json", self.base_url, mbid);
        self.fetch_entity(&url)
    }

    /// Fetch a release by MBID with artist credits, recordings, and media (tracklist).
    pub fn fetch_release(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!(
            "{}/release/{}?inc=recordings+media+artist-credits&fmt=json",
            self.base_url, mbid
        );
        self.fetch_entity(&url)
    }

    /// Generic entity fetch — GET + return raw JSON bytes.
    fn fetch_entity(&self, url: &str) -> Result<MbLookupOutcome> {
        let response = self.agent.get(url).call();

        match response {
            Ok(resp) => {
                let body = resp
                    .into_string()
                    .context("Failed to read MusicBrainz response body")?;
                Ok(MbLookupOutcome::Found(body.into_bytes()))
            }
            Err(ureq::Error::Status(404, _)) => Ok(MbLookupOutcome::NotFound),
            Err(ureq::Error::Status(429, _)) => Ok(MbLookupOutcome::RateLimited),
            Err(ureq::Error::Status(503, _)) => Ok(MbLookupOutcome::ServiceUnavailable),
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                anyhow::bail!("MusicBrainz API returned HTTP {}: {}", code, body);
            }
            Err(e) => Err(anyhow::anyhow!("MusicBrainz network error: {}", e)),
        }
    }
}

// ============================================================================
// Typed Response Structs (parsed on demand from cached JSON)
// ============================================================================

/// A MusicBrainz recording with credits and relations.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct MbReleaseRef {
    pub id: String,
    pub title: Option<String>,
    #[serde(default, rename = "release-group")]
    pub release_group: Option<MbReleaseGroupRef>,
}

/// A release group reference (nested in release).
#[derive(Debug, Deserialize)]
pub struct MbReleaseGroupRef {
    pub id: String,
}

/// An artist credit entry on a recording or release.
#[derive(Debug, Deserialize)]
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
#[derive(Debug, Deserialize)]
pub struct MbArtistRef {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "sort-name")]
    pub sort_name: String,
}

/// A full MusicBrainz artist with aliases (from artist endpoint).
#[derive(Debug, Clone, Deserialize)]
pub struct MbArtist {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "sort-name")]
    pub sort_name: String,
    #[serde(default)]
    pub aliases: Vec<MbAlias>,
}

/// An artist alias (locale-specific name variant).
#[derive(Debug, Clone, Deserialize)]
pub struct MbAlias {
    pub name: String,
    pub locale: Option<String>,
    /// "primary" if this is the primary alias for that locale.
    pub primary: Option<String>,
    /// "Artist name", "Legal name", "Search hint", etc.
    #[serde(rename = "type")]
    pub type_: Option<String>,
}

/// A relation on a recording (artist-recording or work-level).
#[derive(Debug, Deserialize)]
pub struct MbRelation {
    /// Relation type: "vocal", "producer", "remixer", "composer", etc.
    #[serde(rename = "type")]
    pub type_: String,
    /// "backward" = artist→recording direction.
    pub direction: Option<String>,
    /// Qualifiers: ["lead vocals"], ["guitar", "bass"], etc.
    #[serde(default)]
    pub attributes: Vec<String>,
    /// Related artist (present for artist-recording relations).
    pub artist: Option<MbArtistRef>,
}

/// A full MusicBrainz release with artist credits and tracklist (from release endpoint).
#[derive(Debug, Deserialize)]
pub struct MbRelease {
    pub id: String,
    pub title: String,
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<MbArtistCredit>,
    /// Media (discs/sides) with track listings. Empty for old cached JSON
    /// that was fetched without `inc=recordings+media`.
    #[serde(default)]
    pub media: Vec<MbMedium>,
}

/// A medium within a release (CD, vinyl side, digital media, etc.).
#[derive(Debug, Deserialize)]
pub struct MbMedium {
    /// Medium position (1-indexed: disc 1, disc 2, etc.).
    pub position: u32,
    /// Medium format ("CD", "Digital Media", "12\" Vinyl", etc.).
    pub format: Option<String>,
    /// Track list for this medium.
    #[serde(default)]
    pub tracks: Vec<MbTrack>,
}

/// A track within a medium (position + recording reference).
#[derive(Debug, Deserialize)]
pub struct MbTrack {
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
#[derive(Debug, Deserialize)]
pub struct MbTrackRecording {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub length: Option<i64>,
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
pub fn parse_release(raw_json: &[u8]) -> Result<MbRelease> {
    serde_json::from_slice(raw_json).context("Failed to parse cached MusicBrainz release JSON")
}
