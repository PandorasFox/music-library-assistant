//! Genre normalization for collision detection.
//!
//! Normalizes genre tags by:
//! - Converting to lowercase
//! - Normalizing whitespace
//! - Unifying separators ("and" / "&" / "'n'" -> "&")
//! - Normalizing common compound genre spellings

/// Normalize a genre for collision detection.
///
/// - Lowercase
/// - Trim whitespace
/// - Normalize separators: "and" / "&" / "'n'" -> "&"
/// - Normalize common compound genre spellings
/// - Remove extra punctuation
///
/// # Example
///
/// ```
/// use mla_utils::metadata_magic::normalize_genre;
///
/// assert_eq!(normalize_genre("Hip Hop"), "hip-hop");
/// assert_eq!(normalize_genre("Hip-Hop"), "hip-hop");
/// assert_eq!(normalize_genre("Drum and Bass"), "drum & bass");
/// ```
pub fn normalize_genre(s: &str) -> String {
    let mut result = s.to_lowercase().trim().to_string();

    // Normalize whitespace
    result = result.split_whitespace().collect::<Vec<_>>().join(" ");

    // Normalize word separators to "&"
    for pattern in [" and ", " 'n' ", " n ", " 'n ", " n' "] {
        result = result.replace(pattern, " & ");
    }

    // Normalize common compound genres (order matters - longer patterns first)
    let genre_substitutions = [
        // Drum & Bass variants
        (
            ["drum n bass", "drum and bass", "drum'n'bass", "dnb"].as_slice(),
            "drum & bass",
        ),
        // R&B variants
        (
            ["r & b", "rnb", "rhythm and blues", "rhythm & blues"].as_slice(),
            "r&b",
        ),
        // Hip-Hop variants
        (["hip hop", "hiphop"].as_slice(), "hip-hop"),
        // Synth-pop variants
        (["synth pop", "synth-pop"].as_slice(), "synthpop"),
        // Post-punk variants
        (["post punk", "postpunk"].as_slice(), "post-punk"),
        // Trip-hop variants
        (["trip hop", "triphop"].as_slice(), "trip-hop"),
        // Electro variants
        (["electro pop", "electropop"].as_slice(), "electro-pop"),
        // Lo-fi variants
        (["lo fi", "lofi"].as_slice(), "lo-fi"),
        // Alt variants
        (["alternative rock"].as_slice(), "alt-rock"),
        (["alternative"].as_slice(), "alt"),
    ];

    for (patterns, canonical) in genre_substitutions {
        for pattern in patterns {
            if result == *pattern {
                result = canonical.to_string();
                break;
            }
        }
    }

    // Collapse multiple spaces
    while result.contains("  ") {
        result = result.replace("  ", " ");
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_genre_hiphop() {
        assert_eq!(normalize_genre("Hip Hop"), "hip-hop");
        assert_eq!(normalize_genre("Hip-Hop"), "hip-hop");
        assert_eq!(normalize_genre("HipHop"), "hip-hop");
    }

    #[test]
    fn test_normalize_genre_rnb() {
        assert_eq!(normalize_genre("R&B"), "r&b");
        assert_eq!(normalize_genre("R & B"), "r&b");
        assert_eq!(normalize_genre("RnB"), "r&b");
        assert_eq!(normalize_genre("Rhythm and Blues"), "r&b");
    }

    #[test]
    fn test_normalize_genre_dnb() {
        assert_eq!(normalize_genre("Drum and Bass"), "drum & bass");
        assert_eq!(normalize_genre("Drum n Bass"), "drum & bass");
        assert_eq!(normalize_genre("DnB"), "drum & bass");
    }

    #[test]
    fn test_normalize_genre_casing_whitespace() {
        assert_eq!(normalize_genre("  ROCK  "), "rock");
        assert_eq!(normalize_genre("Electronic"), "electronic");
    }

    #[test]
    fn test_normalize_genre_separators() {
        assert_eq!(normalize_genre("Rock and Roll"), "rock & roll");
        assert_eq!(normalize_genre("Rock 'n' Roll"), "rock & roll");
        assert_eq!(normalize_genre("Rock n Roll"), "rock & roll");
    }

    #[test]
    fn test_normalize_genre_other_compounds() {
        assert_eq!(normalize_genre("Synth Pop"), "synthpop");
        assert_eq!(normalize_genre("Post Punk"), "post-punk");
        assert_eq!(normalize_genre("Trip Hop"), "trip-hop");
        assert_eq!(normalize_genre("Lo Fi"), "lo-fi");
        assert_eq!(normalize_genre("Alternative Rock"), "alt-rock");
        assert_eq!(normalize_genre("Alternative"), "alt");
    }
}
