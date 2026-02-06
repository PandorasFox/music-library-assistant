//! Compound Tag Value Detection
//!
//! Detects tag values containing separator characters that should be split
//! into multiple individual tag values.
//!
//! Also handles featuring pattern detection for artist values.

use regex::Regex;

/// Helper methods for compound tag value detection.
///
/// Used by per-inode computations to check individual tag values.
pub struct CompoundTagValue;

impl CompoundTagValue {
    /// Split a value on a separator, trimming whitespace from parts.
    ///
    /// Also strips leading "& " from parts to handle Oxford comma patterns
    /// like "Folk, World, & Country" → ["Folk", "World", "Country"].
    pub fn split_value(value: &str, separator: &str) -> Vec<String> {
        value
            .split(separator)
            .map(|s| s.trim())
            .map(|s| s.strip_prefix("& ").unwrap_or(s))
            .map(|s| s.to_string())
            .filter(|s| !s.is_empty())
            .collect()
    }

    /// Check if a value contains a separator and would split into multiple parts.
    pub fn is_compound(value: &str, separator: &str) -> bool {
        if !value.contains(separator) {
            return false;
        }
        // Must split into at least 2 non-empty parts
        Self::split_value(value, separator).len() > 1
    }
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
    // Require preceding whitespace to avoid Scunthorpe problem
    // (e.g., "Craft Integrated" should NOT match on the "ft" in "Craft")
    let pattern = Regex::new(
        r"(?i)^(.+?)\s+(?:\(?\s*(feat\.?|ft\.?|featuring|vs\.?|with)\s+(.+?)\)?)\s*$"
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
    fn test_detect_featuring_scunthorpe_problem() {
        // Keywords embedded in words should NOT match (require preceding space)
        assert!(detect_featuring_pattern("Craft Integrated").is_none()); // "ft" in "Craft"
        assert!(detect_featuring_pattern("Software Solutions").is_none()); // "ft" in "Software"
        assert!(detect_featuring_pattern("Daft Punk").is_none()); // "ft" in "Daft"
        assert!(detect_featuring_pattern("Leftfield").is_none()); // "ft" in "Leftfield"
        assert!(detect_featuring_pattern("The Gift").is_none()); // "ft" in "Gift"
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
