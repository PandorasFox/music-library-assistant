//! Compound Tag Value Detection
//!
//! Detects tag values containing separator characters that should be split
//! into multiple individual tag values.

use std::collections::HashMap;

use anyhow::Result;

use crate::corpus::db::Database;

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
    pub count: usize,
}

impl CompoundTagValue {
    /// Split a value on a separator, trimming whitespace from parts.
    fn split_value(value: &str, separator: &str) -> Vec<String> {
        value
            .split(separator)
            .map(|s| s.trim().to_string())
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
    db: &Database,
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
                    count,
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
    db: &Database,
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
/// Used when emitting signals to record which tracks are affected.
pub fn get_track_ids_for_compound_value(
    db: &Database,
    tag_name: &str,
    compound_value: &str,
) -> Result<Vec<i64>> {
    db.get_track_ids_for_tag_values(tag_name, &[compound_value])
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
}
