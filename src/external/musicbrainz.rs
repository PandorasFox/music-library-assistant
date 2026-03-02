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

use anyhow::{Context, Result};
use serde::Deserialize;

// ============================================================================
// Client + Outcome Types
// ============================================================================

/// MusicBrainz API client.
pub struct MusicBrainzClient {
    agent: ureq::Agent,
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
    pub fn new() -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("MusicMagic/0.1 (https://github.com/example/musicmagic)")
            .build();
        Self { agent }
    }

    /// Fetch a recording by MBID with artist credits, artist relations, and work relations.
    pub fn fetch_recording(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!(
            "https://musicbrainz.org/ws/2/recording/{}?inc=artist-credits+artist-rels+work-rels+releases&fmt=json",
            mbid
        );
        self.fetch_entity(&url)
    }

    /// Fetch an artist by MBID with aliases.
    pub fn fetch_artist(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!(
            "https://musicbrainz.org/ws/2/artist/{}?inc=aliases&fmt=json",
            mbid
        );
        self.fetch_entity(&url)
    }

    /// Fetch a release by MBID with artist credits.
    pub fn fetch_release(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!(
            "https://musicbrainz.org/ws/2/release/{}?inc=artist-credits&fmt=json",
            mbid
        );
        self.fetch_entity(&url)
    }

    /// Generic entity fetch — GET + return raw JSON bytes.
    fn fetch_entity(&self, url: &str) -> Result<MbLookupOutcome> {
        let response = self.agent.get(url).call();

        match response {
            Ok(resp) => {
                let body = resp.into_string()
                    .context("Failed to read MusicBrainz response body")?;
                Ok(MbLookupOutcome::Found(body.into_bytes()))
            }
            Err(ureq::Error::Status(404, _)) => {
                Ok(MbLookupOutcome::NotFound)
            }
            Err(ureq::Error::Status(429, _)) => {
                Ok(MbLookupOutcome::RateLimited)
            }
            Err(ureq::Error::Status(503, _)) => {
                Ok(MbLookupOutcome::ServiceUnavailable)
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                anyhow::bail!("MusicBrainz API returned HTTP {}: {}", code, body);
            }
            Err(e) => {
                Err(anyhow::anyhow!("MusicBrainz network error: {}", e))
            }
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
#[derive(Debug, Deserialize)]
pub struct MbArtist {
    pub id: String,
    pub name: String,
    #[serde(default, rename = "sort-name")]
    pub sort_name: String,
    #[serde(default)]
    pub aliases: Vec<MbAlias>,
}

/// An artist alias (locale-specific name variant).
#[derive(Debug, Deserialize)]
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

/// A full MusicBrainz release with artist credits (from release endpoint).
#[derive(Debug, Deserialize)]
pub struct MbRelease {
    pub id: String,
    pub title: String,
    #[serde(default, rename = "artist-credit")]
    pub artist_credit: Vec<MbArtistCredit>,
}

// ============================================================================
// Parse Helpers
// ============================================================================

/// Parse cached recording JSON into typed struct.
pub fn parse_recording(raw_json: &[u8]) -> Result<MbRecording> {
    serde_json::from_slice(raw_json)
        .context("Failed to parse cached MusicBrainz recording JSON")
}

/// Parse cached artist JSON into typed struct.
pub fn parse_artist(raw_json: &[u8]) -> Result<MbArtist> {
    serde_json::from_slice(raw_json)
        .context("Failed to parse cached MusicBrainz artist JSON")
}

/// Parse cached release JSON into typed struct.
pub fn parse_release(raw_json: &[u8]) -> Result<MbRelease> {
    serde_json::from_slice(raw_json)
        .context("Failed to parse cached MusicBrainz release JSON")
}
