//! Deezer API client for ISRC-keyed cover art lookup.
//!
//! Deezer's public API is unauthenticated. The `track/isrc:{isrc}` endpoint
//! resolves a 12-character ISRC directly to a track record whose `album` field
//! exposes cover art URLs at multiple resolutions. We use `cover_xl`
//! (1000×1000 JPEG) as the canonical write target.
//!
//! Used as a secondary cover-art source for corpus directories that have
//! tracks tagged with ISRC but no on-disk sidecar and no embedded artwork.
//! Operator-triggered (no auto-add) via the `DeezerArtFetch` witch command.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::external::coverart::{
    detect_format_from_magic, DownloadedImage, ImageFormat,
};

const DEEZER_BASE_URL: &str = "https://api.deezer.com";

// ============================================================================
// Types
// ============================================================================

/// Deezer HTTP client. Clone is cheap (reqwest::Client is Arc internally).
#[derive(Clone)]
pub struct DeezerClient {
    client: reqwest::Client,
}

/// Track record as returned by `GET /track/isrc:{isrc}`.
///
/// Deezer wraps API errors in a top-level `{"error": {...}}` envelope rather
/// than using HTTP status codes; we deserialize via [`DeezerTrackResponse`]
/// to disambiguate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeezerTrack {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub isrc: String,
    pub album: DeezerAlbum,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeezerAlbum {
    pub id: i64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub cover: Option<String>,
    #[serde(default)]
    pub cover_small: Option<String>,
    #[serde(default)]
    pub cover_medium: Option<String>,
    #[serde(default)]
    pub cover_big: Option<String>,
    #[serde(default)]
    pub cover_xl: Option<String>,
}

impl DeezerAlbum {
    /// Best available cover URL, preferring the highest resolution.
    pub fn best_cover_url(&self) -> Option<&str> {
        self.cover_xl
            .as_deref()
            .or(self.cover_big.as_deref())
            .or(self.cover_medium.as_deref())
            .or(self.cover.as_deref())
    }
}

/// Top-level shape of a `/track/isrc:{isrc}` response — either the track
/// payload or an error envelope.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum DeezerTrackResponse {
    Track(DeezerTrack),
    Error { error: DeezerError },
}

#[derive(Debug, Deserialize)]
struct DeezerError {
    #[serde(default)]
    #[serde(rename = "type")]
    error_type: String,
    #[serde(default)]
    message: String,
    #[serde(default)]
    code: i64,
}

// ============================================================================
// Client
// ============================================================================

impl DeezerClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("MusicMagic/0.1 (https://github.com/example/musicmagic)")
            .build()
            .expect("failed to build reqwest client");
        Self { client }
    }

    /// Look up a track by ISRC.
    ///
    /// Returns `Ok(None)` when Deezer has no track for the given ISRC
    /// (`error.code == 800`, "no data"). Returns `Err` for transport
    /// failures, non-200 HTTP statuses, malformed JSON, or unexpected
    /// Deezer error codes.
    pub async fn lookup_track_by_isrc(&self, isrc: &str) -> Result<Option<DeezerTrack>> {
        let trimmed = isrc.trim();
        if trimmed.is_empty() {
            anyhow::bail!("empty ISRC");
        }
        let url = format!("{}/track/isrc:{}", DEEZER_BASE_URL, trimmed);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("Deezer track-by-isrc request failed")?;

        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("Deezer returned unexpected status {status} for ISRC {trimmed}");
        }

        let body = resp
            .text()
            .await
            .context("failed to read Deezer response body")?;

        let parsed: DeezerTrackResponse = serde_json::from_str(&body)
            .with_context(|| format!("failed to parse Deezer JSON for ISRC {trimmed}"))?;

        match parsed {
            DeezerTrackResponse::Track(t) => Ok(Some(t)),
            DeezerTrackResponse::Error { error } => {
                if error.code == 800 {
                    Ok(None)
                } else {
                    anyhow::bail!(
                        "Deezer error code {} ({}): {}",
                        error.code,
                        error.error_type,
                        error.message
                    );
                }
            }
        }
    }

    /// Download a cover image from a Deezer CDN URL.
    ///
    /// Validates magic bytes and returns the image plus its detected format.
    pub async fn download_image(&self, url: &str) -> Result<DownloadedImage> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .context("Deezer image download failed")?;

        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("Deezer image download returned status {status} for {url}");
        }

        let bytes = resp
            .bytes()
            .await
            .context("failed to read Deezer image bytes")?
            .to_vec();

        let format = detect_format_from_magic(&bytes);
        if format == ImageFormat::Unknown {
            anyhow::bail!(
                "Deezer returned non-image response for {url} (first bytes: {:02X?})",
                &bytes[..bytes.len().min(16)]
            );
        }

        Ok(DownloadedImage { bytes, format })
    }
}
