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

    /// Encode a chromaprint fingerprint as the compressed text format expected
    /// by the AcoustID API (chromaprint binary compression + URL-safe base64).
    fn encode_fingerprint(fingerprint: &[u32]) -> String {
        compress_fingerprint(fingerprint, CHROMAPRINT_ALGORITHM)
    }

    /// Look up a fingerprint against AcoustID and return both parsed results and raw JSON.
    pub fn lookup_with_raw(
        &self,
        fingerprint: &[u32],
        duration_secs: u32,
    ) -> Result<(LookupOutcome, Option<Vec<u8>>)> {
        let fp_encoded = Self::encode_fingerprint(fingerprint);

        let response = self
            .agent
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

        let body = response
            .into_string()
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
    let response: AcoustIdResponse =
        serde_json::from_slice(body).context("Failed to parse AcoustID JSON response")?;

    if response.status == "error" {
        let message = response
            .error
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
pub fn find_recording_in_response<'a>(
    response: &'a AcoustIdResponse,
    recording_id: &str,
) -> Option<&'a AcoustIdRecording> {
    response
        .results
        .iter()
        .flat_map(|r| r.recordings.iter())
        .find(|rec| rec.id == recording_id)
}

// ============================================================================
// Chromaprint Fingerprint Compression
// ============================================================================

const CHROMAPRINT_ALGORITHM: u8 = 1;
const MAX_NORMAL_VALUE: u32 = 7; // 2^3 - 1
const NORMAL_BITS: u32 = 3;
const EXCEPTIONAL_BITS: u32 = 5;

/// Compress a raw chromaprint fingerprint (Vec<u32>) into the standard
/// compressed text format accepted by AcoustID.
///
/// Algorithm matches chromaprint's C++ `FingerprintCompressor`:
/// 1. XOR-delta encode consecutive subfingerprints
/// 2. For each delta, extract set-bit gaps, split into normal (3-bit)
///    and exceptional (5-bit) values
/// 3. Pack both arrays into bytes
/// 4. Prepend 4-byte header (algorithm + length)
/// 5. URL-safe base64 encode (no padding)
fn compress_fingerprint(fingerprint: &[u32], algorithm: u8) -> String {
    use base64::Engine;

    let raw = compress_fingerprint_bytes(fingerprint, algorithm);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw)
}

/// Compress to raw bytes (pre-base64). Exposed for testing.
fn compress_fingerprint_bytes(fingerprint: &[u32], algorithm: u8) -> Vec<u8> {
    if fingerprint.is_empty() {
        return vec![algorithm, 0, 0, 0];
    }

    let mut normal_bits: Vec<u32> = Vec::new();
    let mut exceptional_bits: Vec<u32> = Vec::new();

    // First subfingerprint: raw value
    process_subfingerprint(fingerprint[0], &mut normal_bits, &mut exceptional_bits);

    // Subsequent: XOR delta with previous
    for i in 1..fingerprint.len() {
        process_subfingerprint(
            fingerprint[i] ^ fingerprint[i - 1],
            &mut normal_bits,
            &mut exceptional_bits,
        );
    }

    // Header: [algorithm, size_hi, size_mid, size_lo]
    let size = fingerprint.len();
    let mut result = Vec::with_capacity(
        4 + (normal_bits.len() * 3).div_ceil(8) + (exceptional_bits.len() * 5).div_ceil(8),
    );
    result.push(algorithm);
    result.push(((size >> 16) & 0xFF) as u8);
    result.push(((size >> 8) & 0xFF) as u8);
    result.push((size & 0xFF) as u8);

    // Pack normal bits (3 bits per value)
    pack_bits(&normal_bits, NORMAL_BITS, &mut result);
    // Pack exceptional bits (5 bits per value)
    pack_bits(&exceptional_bits, EXCEPTIONAL_BITS, &mut result);

    result
}

/// Extract set-bit gaps from a subfingerprint value, splitting into
/// normal (≤6) and exceptional (≥7, stored as value-7) components.
fn process_subfingerprint(mut x: u32, normal_bits: &mut Vec<u32>, exceptional_bits: &mut Vec<u32>) {
    let mut bit = 1u32;
    let mut last_bit = 0u32;

    while x != 0 {
        if (x & 1) != 0 {
            let gap = bit - last_bit;
            last_bit = bit;
            if gap >= MAX_NORMAL_VALUE {
                normal_bits.push(MAX_NORMAL_VALUE);
                exceptional_bits.push(gap - MAX_NORMAL_VALUE);
            } else {
                normal_bits.push(gap);
            }
        }
        x >>= 1;
        bit += 1;
    }
    normal_bits.push(0); // terminator
}

/// Pack an array of N-bit values into bytes, LSB-first.
fn pack_bits(values: &[u32], bits_per_value: u32, output: &mut Vec<u8>) {
    let mut bit_pos: u32 = 0;
    let mut current_byte: u8 = 0;

    for &value in values {
        let mut v = value;
        let mut remaining = bits_per_value;
        let mut pos_in_byte = bit_pos % 8;

        while remaining > 0 {
            let space = 8 - pos_in_byte;
            let take = remaining.min(space);
            let mask = (1u32 << take) - 1;
            current_byte |= ((v & mask) as u8) << pos_in_byte;
            v >>= take;
            remaining -= take;
            bit_pos += take;
            pos_in_byte += take;

            if pos_in_byte >= 8 {
                output.push(current_byte);
                current_byte = 0;
                pos_in_byte = 0;
            }
        }
    }

    // Flush any remaining partial byte
    if !bit_pos.is_multiple_of(8) {
        output.push(current_byte);
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    // Test cases from chromaprint's test_fingerprint_compressor.cpp.

    #[test]
    fn one_item_one_bit() {
        assert_eq!(
            compress_fingerprint_bytes(&[1], 0), // C++ tests use algorithm 0
            vec![0, 0, 0, 1, 1]
        );
    }

    #[test]
    fn one_item_three_bits() {
        assert_eq!(compress_fingerprint_bytes(&[7], 0), vec![0, 0, 0, 1, 73, 0]);
    }

    #[test]
    fn one_item_one_bit_except() {
        assert_eq!(compress_fingerprint_bytes(&[64], 0), vec![0, 0, 0, 1, 7, 0]);
    }

    #[test]
    fn one_item_one_bit_except2() {
        assert_eq!(
            compress_fingerprint_bytes(&[256], 0),
            vec![0, 0, 0, 1, 7, 2]
        );
    }

    #[test]
    fn two_items() {
        assert_eq!(
            compress_fingerprint_bytes(&[1, 0], 0),
            vec![0, 0, 0, 2, 65, 0]
        );
    }

    #[test]
    fn two_items_no_change() {
        assert_eq!(
            compress_fingerprint_bytes(&[1, 1], 0),
            vec![0, 0, 0, 2, 1, 0]
        );
    }

    #[test]
    fn empty_fingerprint() {
        assert_eq!(compress_fingerprint_bytes(&[], 0), vec![0, 0, 0, 0]);
    }

    #[test]
    fn header_encodes_length_correctly() {
        let fp = vec![0u32; 300];
        let result = compress_fingerprint_bytes(&fp, 1);
        assert_eq!(result[0], 1);
        assert_eq!(result[1], 0); // (300 >> 16)
        assert_eq!(result[2], 1); // (300 >> 8)
        assert_eq!(result[3], 44); // 300 & 0xFF
    }

    #[test]
    fn base64_output_is_url_safe() {
        let fp = vec![0xDEADBEEF, 0xCAFEBABE, 0x12345678];
        let encoded = compress_fingerprint(&fp, CHROMAPRINT_ALGORITHM);
        assert!(!encoded.contains('+'), "contains +: {}", encoded);
        assert!(!encoded.contains('/'), "contains /: {}", encoded);
        assert!(!encoded.ends_with('='), "has padding: {}", encoded);
    }

    #[test]
    fn round_trip_header() {
        let fp = vec![42u32; 10];
        let encoded = compress_fingerprint(&fp, CHROMAPRINT_ALGORITHM);
        use base64::Engine;
        let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&encoded)
            .unwrap();
        assert_eq!(decoded[0], CHROMAPRINT_ALGORITHM);
        let len =
            ((decoded[1] as usize) << 16) | ((decoded[2] as usize) << 8) | (decoded[3] as usize);
        assert_eq!(len, 10);
    }
}
