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
    #[serde(deserialize_with = "deserialize_id")]
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
///
/// Older CAA entries may omit some thumbnail sizes; all fields are optional.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaaThumbnails {
    #[serde(rename = "250", default)]
    pub small: Option<String>,
    #[serde(rename = "500", default)]
    pub medium: Option<String>,
    #[serde(rename = "1200", default)]
    pub large: Option<String>,
}

/// Deserialize `id` from either a JSON number or a string-encoded number.
fn deserialize_id<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    struct IdVisitor;
    impl serde::de::Visitor<'_> for IdVisitor {
        type Value = i64;
        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("integer or string-encoded integer")
        }
        fn visit_i64<E: serde::de::Error>(self, v: i64) -> Result<i64, E> { Ok(v) }
        fn visit_u64<E: serde::de::Error>(self, v: u64) -> Result<i64, E> { Ok(v as i64) }
        fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<i64, E> {
            v.parse().map_err(E::custom)
        }
    }
    d.deserialize_any(IdVisitor)
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

/// Detect image format from magic bytes (file signature).
///
/// This is the authoritative format check — Content-Type headers lie
/// (especially when archive.org returns HTML error pages as 200 OK).
pub fn detect_format_from_magic(bytes: &[u8]) -> ImageFormat {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        ImageFormat::Jpeg
    } else if bytes.starts_with(&[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A]) {
        ImageFormat::Png
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        ImageFormat::Webp
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        ImageFormat::Gif
    } else if bytes.starts_with(b"BM") {
        ImageFormat::Bmp
    } else {
        ImageFormat::Unknown
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
    /// Follows redirects (CAA returns 307 to archive.org). Validates the
    /// response is actually an image via magic bytes — archive.org is known
    /// to return HTML error pages as 200 OK during outages.
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

        let bytes = resp
            .bytes()
            .await
            .context("failed to read CAA image bytes")?
            .to_vec();

        let format = detect_format_from_magic(&bytes);
        if format == ImageFormat::Unknown {
            anyhow::bail!(
                "CAA returned non-image response for {url} (first bytes: {:02X?})",
                &bytes[..bytes.len().min(16)]
            );
        }

        Ok(DownloadedImage { bytes, format })
    }
}
