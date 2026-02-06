//! Compound Tag Value Detection
//!
//! Detects tag values containing separator characters that should be split
//! into multiple individual tag values.
//!
//! Also handles featuring pattern detection for artist values.

use std::collections::HashMap;

use anyhow::Result;
use regex::Regex;

use crate::corpus::db::ReadOnlyDb;

/// A detected compound tag value that should be split.
#[derive(Debug, Clone)]
pub struct CompoundTagValue {
    /// Tag field name (e.g., "genre", "artist")
    pub tag_name: String,
    /// The compound value as stored (e.g., "Rock; Metal")
    pub compound_value: String,
    /// The separator that was detected (e.g., "; ")
    pub separator: String,
    /// The parts after splitting (e.g., ["Rock", "Metal"])
    pub split_parts: Vec<String>,
    /// Number of tracks with this compound value
    pub _count: usize,
}

impl CompoundTagValue {
    /// Split a value on a separator, trimming whitespace from parts.
    ///
    /// Also strips leading "& " from parts to handle Oxford comma patterns
    /// like "Folk, World, & Country" → ["Folk", "World", "Country"].
    fn split_value(value: &str, separator: &str) -> Vec<String> {
        value
            .split(separator)
            .map(|s| s.trim())
            .map(|s| s.strip_prefix("& ").unwrap_or(s))
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Check if a value contains a separator and would split into multiple parts.
    fn is_compound(value: &str, separator: &str) -> bool {
        if !value.contains(separator) {
            return false;
        }
        // Must split into at least 2 non-empty parts
        Self::split_value(value, separator).len() > 1
    }
}

/// Detect compound tag values for a specific tag name and set of separators.
///
/// Returns all tag values that contain any of the given separators and would
/// split into multiple parts.
pub fn detect_compound_values(
    db: &ReadOnlyDb<'_>,
    tag_name: &str,
    separators: &[String],
) -> Result<Vec<CompoundTagValue>> {
    let values = db.get_distinct_tag_values(tag_name)?;

    let mut results = Vec::new();

    for (value, count) in values {
        // Try each separator to see if this value is compound
        for separator in separators {
            if CompoundTagValue::is_compound(&value, separator) {
                let split_parts = CompoundTagValue::split_value(&value, separator);
                results.push(CompoundTagValue {
                    tag_name: tag_name.to_string(),
                    compound_value: value.clone(),
                    separator: separator.clone(),
                    split_parts,
                    _count: count,
                });
                // Only report the first matching separator
                break;
            }
        }
    }

    Ok(results)
}

/// Detect compound values across all configured tag/separator mappings.
///
/// Takes a map of tag_name -> separators and returns all detected compound values.
pub fn detect_all_compound_values(
    db: &ReadOnlyDb<'_>,
    tag_separators: &HashMap<String, Vec<String>>,
) -> Result<Vec<CompoundTagValue>> {
    let mut results = Vec::new();

    for (tag_name, separators) in tag_separators {
        let compounds = detect_compound_values(db, tag_name, separators)?;
        results.extend(compounds);
    }

    Ok(results)
}

/// Get track IDs that have a specific compound tag value.
///
/// Used when emitting signals to record which files are affected.
pub fn get_inodes_for_compound_value(
    db: &ReadOnlyDb<'_>,
    tag_name: &str,
    compound_value: &str,
) -> Result<Vec<i64>> {
    db.get_inodes_for_tag_values(tag_name, &[compound_value])
}

// ============================================================================
// Featuring Pattern Detection
// ============================================================================

/// Detect "feat.", "ft.", "featuring", "with", "vs." patterns in artist values.
///
/// Returns Some((main_artist, featured_artists)) if a featuring pattern is found.
/// Returns None if no featuring pattern is detected.
///
/// Patterns handled:
/// - "Artist A feat. Artist B"
/// - "Artist A ft. Artist B"
/// - "Artist A featuring Artist B"
/// - "Artist A with Artist B" (when followed by artist name)
/// - "Artist A vs. Artist B"
/// - "Artist A vs Artist B"
/// - Parenthetical: "Artist A (feat. Artist B)"
///
/// # Examples
/// ```ignore
/// detect_featuring_pattern("Galantis feat. Dolly Parton")
///     => Some(("Galantis", vec!["Dolly Parton"]))
///
/// detect_featuring_pattern("Skrillex & Diplo with Justin Bieber")
///     => Some(("Skrillex & Diplo", vec!["Justin Bieber"]))
///
/// detect_featuring_pattern("Rinse & Repeat")
///     => None  // No featuring pattern, this is a band name
/// ```
pub fn detect_featuring_pattern(value: &str) -> Option<(String, Vec<String>)> {
    // Pattern matches: feat., ft., featuring, vs., vs, with
    // Case-insensitive, handles parenthetical forms
    //
    // The regex captures:
    // - Group 1: main artist (everything before the pattern)
    // - Group 2: the pattern itself (for debugging, not used in output)
    // - Group 3: featured artists (everything after)
    let pattern = Regex::new(
        r"(?i)^(.+?)\s*(?:\(?\s*(feat\.?|ft\.?|featuring|vs\.?|with)\s+(.+?)\)?)\s*$"
    ).ok()?;

    let caps = pattern.captures(value)?;

    let main_artist = caps.get(1)?.as_str().trim().to_string();
    let featured_str = caps.get(3)?.as_str().trim();

    // Validate: main artist shouldn't be empty after trimming
    if main_artist.is_empty() {
        return None;
    }

    // Featured artists may themselves contain " & " or ", "
    // Split them but keep it simple for now - just return as single string
    // The caller can further split if needed
    let featured = vec![featured_str.to_string()];

    // Validate: featured shouldn't be empty
    if featured.iter().all(|s| s.is_empty()) {
        return None;
    }

    Some((main_artist, featured))
}

/// Detect featuring patterns in all artist values in the corpus.
///
/// This is an additional detection pass beyond separator-based splitting.
/// Returns CompoundTagValue entries for artist values that match featuring
/// patterns like "feat.", "ft.", "featuring", "with", "vs.".
///
/// These are returned as CompoundTagValue entries so they can be processed
/// through the same UI flow as separator-based compounds.
pub fn detect_featuring_compound_values(db: &ReadOnlyDb<'_>) -> Result<Vec<CompoundTagValue>> {
    let values = db.get_distinct_tag_values("artist")?;
    let mut results = Vec::new();

    for (value, count) in values {
        if let Some((main_artist, featured_artists)) = detect_featuring_pattern(&value) {
            // Build split_parts: main artist first, then featured artists
            let mut split_parts = vec![main_artist];
            split_parts.extend(featured_artists);

            // Determine the separator pattern that was matched (for display)
            // We use a simplified representation since the actual pattern varies
            let separator = if value.to_lowercase().contains(" feat") {
                "feat.".to_string()
            } else if value.to_lowercase().contains(" ft") {
                "ft.".to_string()
            } else if value.to_lowercase().contains(" featuring") {
                "featuring".to_string()
            } else if value.to_lowercase().contains(" vs") {
                "vs.".to_string()
            } else if value.to_lowercase().contains(" with ") {
                "with".to_string()
            } else {
                "feat.".to_string() // fallback
            };

            results.push(CompoundTagValue {
                tag_name: "artist".to_string(),
                compound_value: value,
                separator,
                split_parts,
                _count: count,
            });
        }
    }

    Ok(results)
}

/// Check which split parts exist as standalone values in the corpus.
///
/// For artist compound values, this helps determine if we should suggest
/// splitting vs. canonicalizing:
/// - If "Priority" and "TwoThirds" both exist as standalone artists → split
/// - If neither exists as standalone → probably a band name, canonicalize
///
/// Returns the list of parts that were found as standalone values.
pub fn find_matching_standalone_parts(
    db: &ReadOnlyDb<'_>,
    tag_name: &str,
    parts: &[String],
) -> Result<Vec<String>> {
    let mut matching = Vec::new();
    for part in parts {
        if db.tag_value_exists_standalone(tag_name, part)? {
            matching.push(part.clone());
        }
    }
    Ok(matching)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_split_value_semicolon() {
        let parts = CompoundTagValue::split_value("Rock; Metal; Jazz", "; ");
        assert_eq!(parts, vec!["Rock", "Metal", "Jazz"]);
    }

    #[test]
    fn test_split_value_comma() {
        let parts = CompoundTagValue::split_value("Pop, Rock, Alternative", ", ");
        assert_eq!(parts, vec!["Pop", "Rock", "Alternative"]);
    }

    #[test]
    fn test_split_value_slash() {
        let parts = CompoundTagValue::split_value("Electronic/Dance", "/");
        assert_eq!(parts, vec!["Electronic", "Dance"]);
    }

    #[test]
    fn test_split_value_trims_whitespace() {
        let parts = CompoundTagValue::split_value("Rock ;  Metal  ; Jazz", ";");
        assert_eq!(parts, vec!["Rock", "Metal", "Jazz"]);
    }

    #[test]
    fn test_split_value_filters_empty() {
        let parts = CompoundTagValue::split_value("Rock;;Metal", ";");
        assert_eq!(parts, vec!["Rock", "Metal"]);
    }

    #[test]
    fn test_split_value_strips_oxford_comma_ampersand() {
        // Common pattern: "Folk, World, & Country" should become ["Folk", "World", "Country"]
        let parts = CompoundTagValue::split_value("Folk, World, & Country", ",");
        assert_eq!(parts, vec!["Folk", "World", "Country"]);

        // Works with semicolons too
        let parts = CompoundTagValue::split_value("Rock; Pop; & Soul", ";");
        assert_eq!(parts, vec!["Rock", "Pop", "Soul"]);
    }

    #[test]
    fn test_is_compound_true() {
        assert!(CompoundTagValue::is_compound("Rock; Metal", "; "));
        assert!(CompoundTagValue::is_compound("Pop/Rock", "/"));
    }

    #[test]
    fn test_is_compound_false_no_separator() {
        assert!(!CompoundTagValue::is_compound("Rock", "; "));
    }

    #[test]
    fn test_is_compound_false_single_part() {
        // Has separator but only one non-empty part
        assert!(!CompoundTagValue::is_compound("Rock;", ";"));
        assert!(!CompoundTagValue::is_compound(";Rock", ";"));
    }

    // ========================================================================
    // Featuring Pattern Detection Tests
    // ========================================================================

    #[test]
    fn test_detect_featuring_feat() {
        let result = detect_featuring_pattern("Galantis feat. Dolly Parton");
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Galantis");
        assert_eq!(featured, vec!["Dolly Parton"]);
    }

    #[test]
    fn test_detect_featuring_ft() {
        let result = detect_featuring_pattern("Drake ft. Rihanna");
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Drake");
        assert_eq!(featured, vec!["Rihanna"]);
    }

    #[test]
    fn test_detect_featuring_full_word() {
        let result = detect_featuring_pattern("Kanye West featuring Jay-Z");
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Kanye West");
        assert_eq!(featured, vec!["Jay-Z"]);
    }

    #[test]
    fn test_detect_featuring_vs() {
        let result = detect_featuring_pattern("Ken vs. Ryu");
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Ken");
        assert_eq!(featured, vec!["Ryu"]);
    }

    #[test]
    fn test_detect_featuring_with() {
        let result = detect_featuring_pattern("Skrillex & Diplo with Justin Bieber");
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Skrillex & Diplo");
        assert_eq!(featured, vec!["Justin Bieber"]);
    }

    #[test]
    fn test_detect_featuring_parenthetical() {
        let result = detect_featuring_pattern("Major Lazer (feat. DJ Snake)");
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Major Lazer");
        assert_eq!(featured, vec!["DJ Snake"]);
    }

    #[test]
    fn test_detect_featuring_no_pattern() {
        // Band names with " & " should NOT be detected as featuring
        assert!(detect_featuring_pattern("Rinse & Repeat").is_none());
        assert!(detect_featuring_pattern("Simon & Garfunkel").is_none());
        assert!(detect_featuring_pattern("Crosby, Stills & Nash").is_none());
        assert!(detect_featuring_pattern("Priority & TwoThirds").is_none());
    }

    #[test]
    fn test_detect_featuring_case_insensitive() {
        let result = detect_featuring_pattern("Artist A FEAT. Artist B");
        assert!(result.is_some());
        let (main, _) = result.unwrap();
        assert_eq!(main, "Artist A");

        let result = detect_featuring_pattern("Artist A Featuring Artist B");
        assert!(result.is_some());
    }
}
