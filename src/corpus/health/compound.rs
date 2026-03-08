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

/// Detect collaboration keyword patterns in tag values.
///
/// Returns Some((main_part, secondary_parts)) if a collaboration keyword is found.
/// Returns None if no pattern is detected.
///
/// The `keywords` slice controls which keywords to match (e.g., `["feat", "ft", "vs"]`).
/// Each keyword matches with an optional trailing dot, case-insensitively.
///
/// Patterns handled:
/// - "Artist A feat. Artist B"
/// - "Artist A ft Artist B"
/// - "Artist A featuring Artist B"
/// - Parenthetical: "Artist A (feat. Artist B)"
///
/// Requires preceding whitespace to avoid the Scunthorpe problem
/// (e.g., "Craft Integrated" won't match on the "ft" in "Craft").
pub fn detect_featuring_pattern(value: &str, keywords: &[String]) -> Option<(String, Vec<String>)> {
    if keywords.is_empty() {
        return None;
    }

    // Build alternation from keywords, each with optional trailing dot
    let alternation: Vec<String> = keywords
        .iter()
        .map(|kw| format!(r"{}\.?", regex::escape(kw)))
        .collect();
    let keyword_pattern = alternation.join("|");

    let pattern = Regex::new(&format!(
        r"(?i)^(.+?)\s+(?:\(?\s*({})\s+(.+?)\)?)\s*$",
        keyword_pattern
    ))
    .ok()?;

    let caps = pattern.captures(value)?;

    let main_artist = caps.get(1)?.as_str().trim().to_string();
    let featured_str = caps.get(3)?.as_str().trim();

    if main_artist.is_empty() {
        return None;
    }

    let featured = vec![featured_str.to_string()];

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

    fn default_keywords() -> Vec<String> {
        vec![
            "feat".to_string(),
            "featuring".to_string(),
            "ft".to_string(),
            "with".to_string(),
            "vs".to_string(),
        ]
    }

    #[test]
    fn test_detect_featuring_feat() {
        let kw = default_keywords();
        let result = detect_featuring_pattern("Galantis feat. Dolly Parton", &kw);
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Galantis");
        assert_eq!(featured, vec!["Dolly Parton"]);
    }

    #[test]
    fn test_detect_featuring_ft() {
        let kw = default_keywords();
        let result = detect_featuring_pattern("Drake ft. Rihanna", &kw);
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Drake");
        assert_eq!(featured, vec!["Rihanna"]);
    }

    #[test]
    fn test_detect_featuring_full_word() {
        let kw = default_keywords();
        let result = detect_featuring_pattern("Kanye West featuring Jay-Z", &kw);
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Kanye West");
        assert_eq!(featured, vec!["Jay-Z"]);
    }

    #[test]
    fn test_detect_featuring_vs() {
        let kw = default_keywords();
        let result = detect_featuring_pattern("Ken vs. Ryu", &kw);
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Ken");
        assert_eq!(featured, vec!["Ryu"]);
    }

    #[test]
    fn test_detect_featuring_with() {
        let kw = default_keywords();
        let result = detect_featuring_pattern("Skrillex & Diplo with Justin Bieber", &kw);
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Skrillex & Diplo");
        assert_eq!(featured, vec!["Justin Bieber"]);
    }

    #[test]
    fn test_detect_featuring_parenthetical() {
        let kw = default_keywords();
        let result = detect_featuring_pattern("Major Lazer (feat. DJ Snake)", &kw);
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Major Lazer");
        assert_eq!(featured, vec!["DJ Snake"]);
    }

    #[test]
    fn test_detect_featuring_no_pattern() {
        let kw = default_keywords();
        assert!(detect_featuring_pattern("Rinse & Repeat", &kw).is_none());
        assert!(detect_featuring_pattern("Simon & Garfunkel", &kw).is_none());
        assert!(detect_featuring_pattern("Crosby, Stills & Nash", &kw).is_none());
        assert!(detect_featuring_pattern("Priority & TwoThirds", &kw).is_none());
    }

    #[test]
    fn test_detect_featuring_scunthorpe_problem() {
        let kw = default_keywords();
        assert!(detect_featuring_pattern("Craft Integrated", &kw).is_none());
        assert!(detect_featuring_pattern("Software Solutions", &kw).is_none());
        assert!(detect_featuring_pattern("Daft Punk", &kw).is_none());
        assert!(detect_featuring_pattern("Leftfield", &kw).is_none());
        assert!(detect_featuring_pattern("The Gift", &kw).is_none());
    }

    #[test]
    fn test_detect_featuring_case_insensitive() {
        let kw = default_keywords();
        let result = detect_featuring_pattern("Artist A FEAT. Artist B", &kw);
        assert!(result.is_some());
        let (main, _) = result.unwrap();
        assert_eq!(main, "Artist A");

        let result = detect_featuring_pattern("Artist A Featuring Artist B", &kw);
        assert!(result.is_some());
    }

    #[test]
    fn test_detect_featuring_empty_keywords() {
        assert!(detect_featuring_pattern("Artist A feat. Artist B", &[]).is_none());
    }

    #[test]
    fn test_detect_featuring_custom_keywords() {
        let kw = vec!["prod".to_string()];
        let result = detect_featuring_pattern("Track prod. Someone", &kw);
        assert!(result.is_some());
        let (main, featured) = result.unwrap();
        assert_eq!(main, "Track");
        assert_eq!(featured, vec!["Someone"]);
    }
}
