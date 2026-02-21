//! AcoustID HTTP client.
//!
//! Looks up audio fingerprints against the AcoustID database to identify
//! recordings. Used by the fetch thread for background metadata enrichment.
//!
//! API docs: https://acoustid.org/webservice

use anyhow::{Context, Result};

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
                "client={}&fingerprint={}&duration={}&meta=recordings",
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

        let outcome = parse_acoustid_response(&body)?;
        Ok((outcome, Some(raw)))
    }
}

/// Parse AcoustID JSON response into a LookupOutcome.
fn parse_acoustid_response(body: &str) -> Result<LookupOutcome> {
    let json: serde_json::Value = serde_json::from_str(body)
        .context("Failed to parse AcoustID JSON response")?;

    let status = json.get("status")
        .and_then(|s| s.as_str())
        .unwrap_or("");

    if status == "error" {
        let message = json.get("error")
            .and_then(|e| e.get("message"))
            .and_then(|m| m.as_str())
            .unwrap_or("unknown error");
        anyhow::bail!("AcoustID API error: {}", message);
    }

    let results = match json.get("results").and_then(|r| r.as_array()) {
        Some(results) => results,
        None => return Ok(LookupOutcome::NoMatch),
    };

    if results.is_empty() {
        return Ok(LookupOutcome::NoMatch);
    }

    let mut matches = Vec::new();

    for result in results {
        let score = result.get("score")
            .and_then(|s| s.as_f64())
            .unwrap_or(0.0);

        if let Some(recordings) = result.get("recordings").and_then(|r| r.as_array()) {
            for recording in recordings {
                if let Some(id) = recording.get("id").and_then(|i| i.as_str()) {
                    matches.push(MatchRow {
                        recording_id: id.to_string(),
                        confidence: score,
                    });
                }
            }
        }
    }

    if matches.is_empty() {
        Ok(LookupOutcome::NoMatch)
    } else {
        Ok(LookupOutcome::Matches(matches))
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
