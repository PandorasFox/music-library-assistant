//! Cover Art Archive (CAA) client for fetching album artwork.
//!
//! The CAA is hosted by the Internet Archive and provides cover art images
//! linked to MusicBrainz releases. No rate limiting is enforced.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

const CAA_BASE_URL: &str = "https://coverartarchive.org";

// ============================================================================
// Types
// ============================================================================

/// Cover Art Archive HTTP client. Clone is cheap (reqwest::Client is Arc internally).
#[derive(Clone)]
pub struct CoverArtClient {
    client: reqwest::Client,
}

/// Listing of all cover art images for a release.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaaListing {
    pub images: Vec<CaaImage>,
}

/// A single cover art image entry from the CAA listing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaaImage {
    pub id: i64,
    pub types: Vec<String>,
    pub front: bool,
    pub back: bool,
    pub image: String,
    pub thumbnails: CaaThumbnails,
    pub approved: bool,
    #[serde(default)]
    pub comment: String,
}

/// Available thumbnail URLs for a cover art image.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaaThumbnails {
    #[serde(rename = "250")]
    pub small: String,
    #[serde(rename = "500")]
    pub medium: String,
    #[serde(rename = "1200")]
    pub large: String,
}

/// A downloaded image with its detected format.
pub struct DownloadedImage {
    pub bytes: Vec<u8>,
    pub format: ImageFormat,
}

/// Image format detected from HTTP Content-Type or file magic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageFormat {
    Jpeg,
    Png,
    Webp,
    Gif,
    Bmp,
    Unknown,
}

impl ImageFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Gif => "gif",
            Self::Bmp => "bmp",
            Self::Unknown => "bin",
        }
    }
}

/// Detect image format from a Content-Type header value.
pub fn detect_format_from_content_type(content_type: Option<&str>) -> ImageFormat {
    match content_type {
        Some(ct) if ct.contains("image/jpeg") => ImageFormat::Jpeg,
        Some(ct) if ct.contains("image/png") => ImageFormat::Png,
        Some(ct) if ct.contains("image/webp") => ImageFormat::Webp,
        Some(ct) if ct.contains("image/gif") => ImageFormat::Gif,
        Some(ct) if ct.contains("image/bmp") => ImageFormat::Bmp,
        _ => ImageFormat::Unknown,
    }
}

// ============================================================================
// Client
// ============================================================================

impl CoverArtClient {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .user_agent("MusicMagic/0.1 (https://github.com/example/musicmagic)")
            .build()
            .expect("failed to build reqwest client");
        Self { client }
    }

    /// Fetch the CAA image listing for a MusicBrainz release.
    ///
    /// Returns `Ok(None)` if the release has no cover art (404).
    pub async fn fetch_listing(&self, release_id: &str) -> Result<Option<CaaListing>> {
        let url = format!("{}/release/{}/", CAA_BASE_URL, release_id);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .context("CAA listing request failed")?;

        match resp.status().as_u16() {
            200 => {
                let listing: CaaListing = resp
                    .json()
                    .await
                    .context("failed to parse CAA listing JSON")?;
                Ok(Some(listing))
            }
            404 => Ok(None),
            status => {
                anyhow::bail!("CAA listing returned unexpected status {status} for {release_id}");
            }
        }
    }

    /// Download a cover art image from its full URL.
    ///
    /// Follows redirects (CAA returns 307 to archive.org). Detects format
    /// from the Content-Type response header.
    pub async fn download_image(&self, url: &str) -> Result<DownloadedImage> {
        let resp = self
            .client
            .get(url)
            .send()
            .await
            .context("CAA image download failed")?;

        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("CAA image download returned status {status} for {url}");
        }

        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        let format = detect_format_from_content_type(content_type.as_deref());

        let bytes = resp
            .bytes()
            .await
            .context("failed to read CAA image bytes")?
            .to_vec();

        Ok(DownloadedImage { bytes, format })
    }
}
