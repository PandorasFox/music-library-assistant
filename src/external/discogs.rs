//! Discogs HTTP client and typed response structures.
//!
//! Discogs's editorial `Genre` and `Style` fields are the strongest per-release
//! genre signal available. We reach them by first discovering the MB→Discogs
//! URL relationship (Phase 3, populated in `mb_release_discogs_links`), then
//! fetching the Discogs release JSON here and caching it in
//! `discogs_release_cache`. The Phase 5 `DeriveDiscogsGenreLedger` computation
//! consumes the cache.
//!
//! ## Authentication & rate limits
//!
//! Discogs offers two tiers:
//!   - **Personal access token** (header `Authorization: Discogs token=...`):
//!     60 requests/min, sustained.
//!   - **Unauthenticated**: 25/min, often unreliable in practice.
//!
//! We require a token (empty config disables Discogs entirely — the scheduler
//! skips the queue). The token is sent on every request; the API also requires
//! a descriptive `User-Agent` header.
//!
//! API docs: https://www.discogs.com/developers

use anyhow::{Context, Result};
use serde::Deserialize;

// ============================================================================
// Typed Discogs Response Structs
// ============================================================================

/// Top-level Discogs release response — only the fields we need for genre
/// extraction. The actual response is huge (tracklist, artist credits, images,
/// notes, etc.); we deliberately ignore most of it.
#[derive(Debug, Clone, Deserialize)]
pub struct DiscogsRelease {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    /// Discogs `Genre` array (umbrella categories like "Electronic", "Rock").
    #[serde(default)]
    pub genres: Vec<String>,
    /// Discogs `Style` array (specific subgenres like "Drum n Bass", "Techno").
    #[serde(default)]
    pub styles: Vec<String>,
}

/// Parse cached release JSON.
pub fn parse_release(raw_json: &[u8]) -> Result<DiscogsRelease> {
    serde_json::from_slice(raw_json).context("Failed to parse cached Discogs release JSON")
}

// ============================================================================
// Client + Outcome Types
// ============================================================================

/// Discogs API client.
#[derive(Clone)]
pub struct DiscogsClient {
    client: reqwest::Client,
    token: String,
    /// Base URL — defaults to `https://api.discogs.com`. Configurable so tests
    /// can point at a local mock.
    base_url: String,
}

/// Outcome of a single Discogs API lookup. Mirrors `MbLookupOutcome` so the
/// scheduler can dispatch on a uniform shape.
pub enum DiscogsLookupOutcome {
    /// Entity found — raw JSON bytes for cache storage.
    Found(Vec<u8>),
    /// 404 — entity does not exist.
    NotFound,
    /// 429 — rate limited.
    RateLimited,
}

impl DiscogsClient {
    /// Construct a new client. An empty token is **not** disabled here — the
    /// scheduler is responsible for refusing to enqueue Discogs work when the
    /// configured token is empty. The client itself sends whatever it's given.
    pub fn new(token: String) -> Self {
        Self::with_base_url(token, "https://api.discogs.com")
    }

    pub fn with_base_url(token: String, base_url: &str) -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(15))
            .user_agent("MusicMagic/0.1 +https://github.com/example/musicmagic")
            .build()
            .expect("failed to build reqwest client");
        Self {
            client,
            token,
            base_url: base_url.trim_end_matches('/').to_string(),
        }
    }

    /// Fetch a Discogs release by id (numeric, as a string).
    pub async fn fetch_release(&self, release_id: &str) -> Result<DiscogsLookupOutcome> {
        let url = format!("{}/releases/{}", self.base_url, release_id);
        self.fetch_entity(&url).await
    }

    async fn fetch_entity(&self, url: &str) -> Result<DiscogsLookupOutcome> {
        let mut req = self.client.get(url);
        if !self.token.is_empty() {
            req = req.header(
                reqwest::header::AUTHORIZATION,
                format!("Discogs token={}", self.token),
            );
        }

        let response = req.send().await;
        match response {
            Ok(resp) => {
                let status = resp.status();
                if status == reqwest::StatusCode::NOT_FOUND {
                    return Ok(DiscogsLookupOutcome::NotFound);
                }
                if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
                    return Ok(DiscogsLookupOutcome::RateLimited);
                }
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    anyhow::bail!("Discogs API returned HTTP {}: {}", status, body);
                }
                let body = resp
                    .bytes()
                    .await
                    .context("Failed to read Discogs response body")?;

                // Sanity-check: Discogs responses always start with '{'. A
                // proxy misconfiguration would return HTML 200, which we
                // refuse to cache.
                if body.first() != Some(&b'{') {
                    let preview: String =
                        body.iter().take(120).map(|&b| b as char).collect();
                    anyhow::bail!(
                        "Discogs response is not JSON (got {} bytes starting with: {})",
                        body.len(),
                        preview.trim(),
                    );
                }
                Ok(DiscogsLookupOutcome::Found(body.to_vec()))
            }
            Err(e) => Err(anyhow::anyhow!("Discogs network error: {}", e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_release_extracts_genres_and_styles() {
        let json = br#"{
            "id": 42,
            "title": "Some Release",
            "genres": ["Electronic", "Rock"],
            "styles": ["Drum n Bass", "Techno"]
        }"#;
        let r = parse_release(json).expect("parse ok");
        assert_eq!(r.id, 42);
        assert_eq!(r.title, "Some Release");
        assert_eq!(r.genres, vec!["Electronic", "Rock"]);
        assert_eq!(r.styles, vec!["Drum n Bass", "Techno"]);
    }

    #[test]
    fn parse_release_tolerates_missing_optional_fields() {
        let json = br#"{ "id": 7, "genres": [], "styles": [] }"#;
        let r = parse_release(json).expect("parse ok");
        assert_eq!(r.id, 7);
        assert!(r.title.is_empty());
    }

}
