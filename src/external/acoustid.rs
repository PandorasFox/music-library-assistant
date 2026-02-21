//! AcoustID HTTP client and typed response structures.
//!
//! Looks up audio fingerprints against the AcoustID database to identify
//! recordings. Used by the fetch thread for background metadata enrichment.
//!
//! ## Response Types
//!
//! AcoustID returns MusicBrainz MBIDs but uses its **own JSON serialization
//! format** — different field names and nesting from the MusicBrainz API.
//! We define our own serde structs here rather than using musicbrainz_rs types.
//!
//! API docs: https://acoustid.org/webservice

use anyhow::{Context, Result};
use serde::Deserialize;

// ============================================================================
// Typed AcoustID Response Structs
// ============================================================================

/// Top-level AcoustID API response.
#[derive(Debug, Deserialize)]
pub struct AcoustIdResponse {
    pub status: String,
    pub error: Option<AcoustIdError>,
    #[serde(default)]
    pub results: Vec<AcoustIdResult>,
}

/// AcoustID API error detail.
#[derive(Debug, Deserialize)]
pub struct AcoustIdError {
    pub message: String,
}

/// One fingerprint match result (may contain multiple recordings).
#[derive(Debug, Deserialize)]
pub struct AcoustIdResult {
    /// Match confidence 0.0–1.0.
    pub score: f64,
    #[serde(default)]
    pub recordings: Vec<AcoustIdRecording>,
}

/// A MusicBrainz recording returned by AcoustID.
#[derive(Debug, Deserialize)]
pub struct AcoustIdRecording {
    /// MusicBrainz recording MBID.
    pub id: String,
    pub title: Option<String>,
    #[serde(default)]
    pub artists: Vec<AcoustIdArtist>,
    /// Present if meta includes +releases.
    #[serde(default)]
    pub releases: Vec<AcoustIdRelease>,
    /// Present if meta includes +releasegroups.
    #[serde(default)]
    pub releasegroups: Vec<AcoustIdReleaseGroup>,
}

/// An artist credit in AcoustID's format.
#[derive(Debug, Deserialize)]
pub struct AcoustIdArtist {
    pub name: String,
    /// Join phrase between this artist and the next (e.g., " & ", " feat. ").
    #[serde(default)]
    pub joinphrase: Option<String>,
}

/// A MusicBrainz release returned by AcoustID.
#[derive(Debug, Deserialize)]
pub struct AcoustIdRelease {
    /// MusicBrainz release MBID.
    pub id: String,
    pub title: Option<String>,
}

/// A MusicBrainz release group returned by AcoustID.
#[derive(Debug, Deserialize)]
pub struct AcoustIdReleaseGroup {
    /// MusicBrainz release group MBID.
    pub id: String,
}

// ============================================================================
// Client + Outcome Types
// ============================================================================

/// AcoustID API client.
pub struct AcoustIDClient {
    api_key: String,
    agent: ureq::Agent,
}

/// A single match row to be written to external_matches.
pub struct MatchRow {
    pub recording_id: String,
    pub confidence: f64,
}

/// Distinguishes API-level outcomes from transport errors.
pub enum LookupOutcome {
    /// Matches found (may be empty vec if acoustid exists but has no linked recordings).
    Matches(Vec<MatchRow>),
    /// No results at all for this fingerprint.
    NoMatch,
    /// Rate limited — caller should retry later.
    RateLimited,
}

impl AcoustIDClient {
    pub fn new(api_key: String) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout(std::time::Duration::from_secs(10))
            .build();
        Self { api_key, agent }
    }

    /// Encode a chromaprint fingerprint as the text format expected by AcoustID API.
    ///
    /// AcoustID expects the fingerprint as a base64-like compressed string.
    /// The `rusty-chromaprint` crate provides `fingerprint_to_raw` which gives Vec<u32>;
    /// we need to convert this to the standard chromaprint text encoding.
    fn encode_fingerprint(fingerprint: &[u32]) -> String {
        // The AcoustID API accepts raw fingerprint data as comma-separated integers
        // when using the `fingerprint` parameter. However, the standard format
        // is the compressed chromaprint string.
        //
        // Since we store raw u32 values, we use rusty-chromaprint's encoding
        // if available, or fall back to the raw integer list approach.
        //
        // For now, use the comma-separated integer format which the API accepts
        // via the `fingerprint` parameter with `format=raw`.
        fingerprint
            .iter()
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Look up a fingerprint against AcoustID and return both parsed results and raw JSON.
    pub fn lookup_with_raw(
        &self,
        fingerprint: &[u32],
        duration_secs: u32,
    ) -> Result<(LookupOutcome, Option<Vec<u8>>)> {
        let fp_encoded = Self::encode_fingerprint(fingerprint);

        let response = self.agent
            .post("https://api.acoustid.org/v2/lookup")
            .set("Content-Type", "application/x-www-form-urlencoded")
            .send_string(&format!(
                "client={}&fingerprint={}&duration={}&meta=recordings+releases+releasegroups",
                urlencoded(&self.api_key),
                urlencoded(&fp_encoded),
                duration_secs,
            ));

        let response = match response {
            Ok(resp) => resp,
            Err(ureq::Error::Status(429, _)) => {
                return Ok((LookupOutcome::RateLimited, None));
            }
            Err(ureq::Error::Status(code, resp)) => {
                let body = resp.into_string().unwrap_or_default();
                anyhow::bail!("AcoustID API returned HTTP {}: {}", code, body);
            }
            Err(e) => {
                return Err(anyhow::anyhow!("AcoustID network error: {}", e));
            }
        };

        let body = response.into_string()
            .context("Failed to read AcoustID response body")?;
        let raw = body.as_bytes().to_vec();

        let outcome = parse_acoustid_response(&raw)?;
        Ok((outcome, Some(raw)))
    }
}

// ============================================================================
// Response Parsing
// ============================================================================

/// Parse AcoustID JSON response bytes into a typed `AcoustIdResponse`.
pub fn parse_acoustid_response(body: &[u8]) -> Result<LookupOutcome> {
    let response: AcoustIdResponse = serde_json::from_slice(body)
        .context("Failed to parse AcoustID JSON response")?;

    if response.status == "error" {
        let message = response.error
            .map(|e| e.message)
            .unwrap_or_else(|| "unknown error".to_string());
        anyhow::bail!("AcoustID API error: {}", message);
    }

    if response.results.is_empty() {
        return Ok(LookupOutcome::NoMatch);
    }

    let mut matches = Vec::new();

    for result in &response.results {
        for recording in &result.recordings {
            matches.push(MatchRow {
                recording_id: recording.id.clone(),
                confidence: result.score,
            });
        }
    }

    if matches.is_empty() {
        Ok(LookupOutcome::NoMatch)
    } else {
        Ok(LookupOutcome::Matches(matches))
    }
}

/// Find a specific recording by MBID in a stored AcoustID response.
///
/// Used by the signal derivation computation to extract metadata for
/// the matched recording from the stored raw response JSON.
pub fn find_recording_in_response<'a>(response: &'a AcoustIdResponse, recording_id: &str) -> Option<&'a AcoustIdRecording> {
    response.results.iter()
        .flat_map(|r| r.recordings.iter())
        .find(|rec| rec.id == recording_id)
}

/// Minimal URL encoding for form parameters.
fn urlencoded(s: &str) -> String {
    let mut result = String::with_capacity(s.len() + 16);
    for c in s.chars() {
        match c {
            'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => result.push(c),
            ' ' => result.push('+'),
            _ => {
                let mut buf = [0u8; 4];
                let encoded = c.encode_utf8(&mut buf);
                for byte in encoded.bytes() {
                    result.push('%');
                    result.push_str(&format!("{:02X}", byte));
                }
            }
        }
    }
    result
}
