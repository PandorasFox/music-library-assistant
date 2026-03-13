//! MusicBrainz API client and server-side cache loading.
//!
//! Pure data types, parse helpers, and locale resolution are re-exported from mm-meta.
//! This module adds the HTTP client and DB-backed cache loading.

use std::collections::HashMap;

use anyhow::{Context, Result};

// Re-export all data types and functions from mm-meta
pub use mm_meta::external::musicbrainz::*;

// ============================================================================
// Client + Outcome Types (server-only)
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
// Batch Cache Loading (server-only — needs ReadOnlyDb)
// ============================================================================

/// Load releases and recordings by ID from the DB cache, chasing artist references.
///
/// Designed to run inside a `cache.query()` closure. Silently skips
/// entries where the cache is missing or JSON fails to parse.
pub fn load_mb_cache_bundle(
    db: &crate::db::queries::ReadOnlyDb<'_>,
    release_ids: &[String],
    recording_ids: &[String],
) -> MbCacheBundle {
    let mut releases = HashMap::new();
    let mut recordings = HashMap::new();
    let mut artists = HashMap::new();

    // Load releases + their credit artists
    for rid in release_ids {
        if let Ok(Some((json, _))) = db.get_mb_release_cache(rid) {
            if let Ok(rel) = parse_release(&json) {
                load_credit_artists(&rel.artist_credit, db, &mut artists);
                releases.insert(rid.clone(), rel);
            }
        }
    }

    // Load recordings + their credit/relation artists
    for rid in recording_ids {
        if let Ok(Some((json, _))) = db.get_mb_recording_cache(rid) {
            if let Ok(rec) = parse_recording(&json) {
                load_credit_artists(&rec.artist_credit, db, &mut artists);
                for relation in &rec.relations {
                    if let Some(ref ra) = relation.artist {
                        load_artist_if_absent(&ra.id, db, &mut artists);
                    }
                }
                recordings.insert(rid.clone(), rec);
            }
        }
    }

    MbCacheBundle {
        releases,
        recordings,
        artists,
    }
}

/// Load artists from credit entries into the map, skipping already-loaded ones.
fn load_credit_artists(
    credits: &[MbArtistCredit],
    db: &crate::db::queries::ReadOnlyDb<'_>,
    artists: &mut HashMap<String, MbArtist>,
) {
    for credit in credits {
        load_artist_if_absent(&credit.artist.id, db, artists);
    }
}

/// Load a single artist into the map if not already present.
fn load_artist_if_absent(
    artist_id: &str,
    db: &crate::db::queries::ReadOnlyDb<'_>,
    artists: &mut HashMap<String, MbArtist>,
) {
    if artists.contains_key(artist_id) {
        return;
    }
    if let Ok(Some((json, _))) = db.get_mb_artist_cache(artist_id) {
        if let Ok(a) = parse_artist(&json) {
            artists.insert(artist_id.to_string(), a);
        }
    }
}
