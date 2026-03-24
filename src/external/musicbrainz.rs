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

/// MusicBrainz API client (async reqwest). Clone is cheap (reqwest::Client is Arc internally).
#[derive(Clone)]
pub struct MusicBrainzClient {
    client: reqwest::Client,
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
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("MusicMagic/0.1 (https://github.com/example/musicmagic)")
            .build()
            .expect("failed to build reqwest client");
        Self {
            client,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Fetch a recording by MBID with artist credits, artist relations, and work relations.
    pub async fn fetch_recording(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!(
            "{}/recording/{}?inc=artist-credits+artist-rels+work-rels+releases&fmt=json",
            self.base_url, mbid
        );
        self.fetch_entity(&url).await
    }

    /// Fetch an artist by MBID with aliases.
    pub async fn fetch_artist(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!("{}/artist/{}?inc=aliases&fmt=json", self.base_url, mbid);
        self.fetch_entity(&url).await
    }

    /// Fetch a release by MBID with artist credits, recordings, and media (tracklist).
    pub async fn fetch_release(&self, mbid: &str) -> Result<MbLookupOutcome> {
        let url = format!(
            "{}/release/{}?inc=recordings+media+artist-credits&fmt=json",
            self.base_url, mbid
        );
        self.fetch_entity(&url).await
    }

    /// Generic entity fetch — GET + validate JSON + return raw bytes.
    async fn fetch_entity(&self, url: &str) -> Result<MbLookupOutcome> {
        let response = self.client.get(url).send().await;

        match response {
            Ok(resp) => {
                let status = resp.status();
                if status == reqwest::StatusCode::NOT_FOUND {
                    return Ok(MbLookupOutcome::NotFound);
                }
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    return Ok(MbLookupOutcome::RateLimited);
                }
                if status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
                    return Ok(MbLookupOutcome::ServiceUnavailable);
                }
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    anyhow::bail!("MusicBrainz API returned HTTP {}: {}", status, body);
                }
                let body = resp
                    .bytes()
                    .await
                    .context("Failed to read MusicBrainz response body")?;

                // Validate response is JSON, not an HTML error page.
                // MB API responses always start with '{'. A web frontend
                // misconfiguration (wrong base_url) returns HTML with 200 OK.
                if body.first() != Some(&b'{') {
                    let preview: String = body.iter()
                        .take(120)
                        .map(|&b| b as char)
                        .collect();
                    anyhow::bail!(
                        "MusicBrainz response is not JSON (got {} bytes starting with: {})",
                        body.len(),
                        preview.trim(),
                    );
                }

                Ok(MbLookupOutcome::Found(body.to_vec()))
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
    let mut rec_missing = 0usize;
    let mut rec_parse_fail = 0usize;
    for rid in recording_ids {
        match db.get_mb_recording_cache(rid) {
            Ok(Some((json, _))) => match parse_recording(&json) {
                Ok(rec) => {
                    load_credit_artists(&rec.artist_credit, db, &mut artists);
                    for relation in &rec.relations {
                        if let Some(ref ra) = relation.artist {
                            load_artist_if_absent(&ra.id, db, &mut artists);
                        }
                    }
                    recordings.insert(rid.clone(), rec);
                }
                Err(e) => {
                    if rec_parse_fail == 0 {
                        eprintln!("[MB-CACHE] recording parse fail for {rid}: {e}");
                    }
                    rec_parse_fail += 1;
                }
            },
            Ok(None) => { rec_missing += 1; }
            Err(e) => {
                if rec_missing + rec_parse_fail == 0 {
                    eprintln!("[MB-CACHE] recording db error for {rid}: {e}");
                }
                rec_missing += 1;
            }
        }
    }
    if rec_missing > 0 || rec_parse_fail > 0 {
        eprintln!(
            "[MB-CACHE] recordings: {} loaded, {} missing, {} parse failures (of {} requested)",
            recordings.len(), rec_missing, rec_parse_fail, recording_ids.len()
        );
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
