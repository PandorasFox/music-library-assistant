//! Path-tag schema: template syntax for expressing expected file path structure
//! in terms of tag values.
//!
//! Template syntax:
//! - `$TAGNAME` or `${TAGNAME}` — required tag placeholder (case insensitive in tag name)
//! - `$[literal $TAGNAME]` — optional group: present only if TAGNAME has a value
//! - `/` — segment boundary (path separator)
//! - Everything else — literal text
//!
//! Example: `$LABEL/$CATALOGNUMBER/$ARTIST$[ - $ALBUM] - $TITLE`

use std::collections::HashMap;
use std::fmt;

// ============================================================================
// Types
// ============================================================================

/// A parsed path-tag schema.
#[derive(Debug, Clone)]
pub struct PathTagSchema {
    /// Raw template string for display/serialization.
    pub template: String,
    /// Parsed segments (split on `/`).
    pub segments: Vec<SchemaSegment>,
}

/// A single path segment within a schema (between `/` separators).
#[derive(Debug, Clone)]
pub struct SchemaSegment {
    pub parts: Vec<SchemaPart>,
}

/// A part of a schema segment.
#[derive(Debug, Clone)]
pub enum SchemaPart {
    /// Literal text that must match exactly.
    Literal(String),
    /// A required tag placeholder. Tag name stored uppercase.
    RequiredTag(String),
    /// An optional group: if the tag is present, expect the literal prefix(es) too.
    /// If the tag is absent, the entire group is skipped during matching.
    OptionalGroup {
        /// The parts within the group (literals + one tag).
        parts: Vec<SchemaPart>,
        /// The tag name that determines presence (uppercase).
        tag_name: String,
    },
}

/// Result of matching a path against a schema.
#[derive(Debug, Clone)]
pub enum PathSchemaMatchResult {
    /// Path matches the schema. Contains extracted tag name → value pairs.
    Match(HashMap<String, String>),
    /// Path does not match the expected structure.
    StructureMismatch(String),
}

/// Error during schema template parsing.
#[derive(Debug, Clone)]
pub enum SchemaParseError {
    EmptyTemplate,
    UnterminatedBrace(usize),
    UnterminatedOptionalGroup(usize),
    EmptyTagName(usize),
    NestedOptionalGroup(usize),
    NoTagInOptionalGroup(usize),
    MultipleTagsInOptionalGroup(usize),
}

impl fmt::Display for SchemaParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyTemplate => write!(f, "empty template"),
            Self::UnterminatedBrace(pos) => write!(f, "unterminated ${{}} at position {}", pos),
            Self::UnterminatedOptionalGroup(pos) => {
                write!(f, "unterminated $[] at position {}", pos)
            }
            Self::EmptyTagName(pos) => write!(f, "empty tag name at position {}", pos),
            Self::NestedOptionalGroup(pos) => write!(f, "nested $[] at position {}", pos),
            Self::NoTagInOptionalGroup(pos) => write!(f, "no tag in $[] at position {}", pos),
            Self::MultipleTagsInOptionalGroup(pos) => {
                write!(f, "multiple tags in $[] at position {}", pos)
            }
        }
    }
}

impl std::error::Error for SchemaParseError {}

// ============================================================================
// Parser
// ============================================================================

/// Parse a template string into a PathTagSchema.
pub fn parse_path_schema(template: &str) -> Result<PathTagSchema, SchemaParseError> {
    if template.is_empty() {
        return Err(SchemaParseError::EmptyTemplate);
    }

    // Split on `/` to get raw segment strings, then parse each.
    let raw_segments: Vec<&str> = template.split('/').collect();
    let mut segments = Vec::with_capacity(raw_segments.len());

    // Track character offset for error positions across segments.
    let mut global_offset = 0;

    for raw in &raw_segments {
        let parts = parse_segment(raw, global_offset)?;
        segments.push(SchemaSegment { parts });
        global_offset += raw.len() + 1; // +1 for the `/`
    }

    Ok(PathTagSchema {
        template: template.to_string(),
        segments,
    })
}

/// Parse a single segment string into parts.
fn parse_segment(segment: &str, base_offset: usize) -> Result<Vec<SchemaPart>, SchemaParseError> {
    let mut parts = Vec::new();
    let chars: Vec<char> = segment.chars().collect();
    let mut i = 0;
    let mut literal_buf = String::new();

    while i < chars.len() {
        if chars[i] == '$' {
            // Flush literal buffer.
            if !literal_buf.is_empty() {
                parts.push(SchemaPart::Literal(std::mem::take(&mut literal_buf)));
            }

            if i + 1 >= chars.len() {
                // Trailing `$` — treat as literal.
                literal_buf.push('$');
                i += 1;
                continue;
            }

            match chars[i + 1] {
                '{' => {
                    // ${TAGNAME} — braced tag reference.
                    let start = i;
                    i += 2; // skip `${`
                    let tag_start = i;
                    while i < chars.len() && chars[i] != '}' {
                        i += 1;
                    }
                    if i >= chars.len() {
                        return Err(SchemaParseError::UnterminatedBrace(base_offset + start));
                    }
                    let tag_name: String = chars[tag_start..i].iter().collect();
                    let tag_name = tag_name.trim().to_uppercase();
                    if tag_name.is_empty() {
                        return Err(SchemaParseError::EmptyTagName(base_offset + start));
                    }
                    parts.push(SchemaPart::RequiredTag(tag_name));
                    i += 1; // skip `}`
                }
                '[' => {
                    // $[...] — optional group.
                    let group_start = i;
                    i += 2; // skip `$[`
                    let content_start = i;
                    let mut depth = 1;
                    while i < chars.len() && depth > 0 {
                        if chars[i] == '$' && i + 1 < chars.len() && chars[i + 1] == '[' {
                            return Err(SchemaParseError::NestedOptionalGroup(base_offset + i));
                        }
                        if chars[i] == ']' {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        i += 1;
                    }
                    if depth > 0 {
                        return Err(SchemaParseError::UnterminatedOptionalGroup(
                            base_offset + group_start,
                        ));
                    }
                    let content: String = chars[content_start..i].iter().collect();
                    i += 1; // skip `]`

                    // Parse the group content for literals and exactly one tag.
                    let group_parts =
                        parse_optional_group_content(&content, base_offset + content_start)?;

                    // Find the tag name in the group.
                    let mut tag_name = None;
                    let mut tag_count = 0;
                    for part in &group_parts {
                        if let SchemaPart::RequiredTag(name) = part {
                            tag_name = Some(name.clone());
                            tag_count += 1;
                        }
                    }

                    if tag_count == 0 {
                        return Err(SchemaParseError::NoTagInOptionalGroup(
                            base_offset + group_start,
                        ));
                    }
                    if tag_count > 1 {
                        return Err(SchemaParseError::MultipleTagsInOptionalGroup(
                            base_offset + group_start,
                        ));
                    }

                    parts.push(SchemaPart::OptionalGroup {
                        parts: group_parts,
                        tag_name: tag_name.unwrap(),
                    });
                }
                c if c.is_alphanumeric() || c == '_' => {
                    // $TAGNAME — bare tag reference.
                    i += 1; // skip `$`
                    let tag_start = i;
                    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                        i += 1;
                    }
                    let tag_name: String = chars[tag_start..i].iter().collect();
                    let tag_name = tag_name.trim().to_uppercase();
                    if tag_name.is_empty() {
                        return Err(SchemaParseError::EmptyTagName(base_offset + tag_start));
                    }
                    parts.push(SchemaPart::RequiredTag(tag_name));
                }
                _ => {
                    // `$` followed by non-alphanum, non-brace, non-bracket: literal `$`.
                    literal_buf.push('$');
                    i += 1;
                }
            }
        } else {
            literal_buf.push(chars[i]);
            i += 1;
        }
    }

    // Flush trailing literal.
    if !literal_buf.is_empty() {
        parts.push(SchemaPart::Literal(literal_buf));
    }

    Ok(parts)
}

/// Parse content inside an optional group `$[...]`.
/// Only allows literals and bare/braced tag references (no nested optional groups).
fn parse_optional_group_content(
    content: &str,
    base_offset: usize,
) -> Result<Vec<SchemaPart>, SchemaParseError> {
    let mut parts = Vec::new();
    let chars: Vec<char> = content.chars().collect();
    let mut i = 0;
    let mut literal_buf = String::new();

    while i < chars.len() {
        if chars[i] == '$' {
            if !literal_buf.is_empty() {
                parts.push(SchemaPart::Literal(std::mem::take(&mut literal_buf)));
            }

            if i + 1 >= chars.len() {
                literal_buf.push('$');
                i += 1;
                continue;
            }

            match chars[i + 1] {
                '{' => {
                    let start = i;
                    i += 2;
                    let tag_start = i;
                    while i < chars.len() && chars[i] != '}' {
                        i += 1;
                    }
                    if i >= chars.len() {
                        return Err(SchemaParseError::UnterminatedBrace(base_offset + start));
                    }
                    let tag_name: String = chars[tag_start..i].iter().collect();
                    let tag_name = tag_name.trim().to_uppercase();
                    if tag_name.is_empty() {
                        return Err(SchemaParseError::EmptyTagName(base_offset + start));
                    }
                    parts.push(SchemaPart::RequiredTag(tag_name));
                    i += 1;
                }
                '[' => {
                    return Err(SchemaParseError::NestedOptionalGroup(base_offset + i));
                }
                c if c.is_alphanumeric() || c == '_' => {
                    i += 1;
                    let tag_start = i;
                    while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                        i += 1;
                    }
                    let tag_name: String = chars[tag_start..i].iter().collect();
                    let tag_name = tag_name.trim().to_uppercase();
                    if tag_name.is_empty() {
                        return Err(SchemaParseError::EmptyTagName(base_offset + tag_start));
                    }
                    parts.push(SchemaPart::RequiredTag(tag_name));
                }
                _ => {
                    literal_buf.push('$');
                    i += 1;
                }
            }
        } else {
            literal_buf.push(chars[i]);
            i += 1;
        }
    }

    if !literal_buf.is_empty() {
        parts.push(SchemaPart::Literal(literal_buf));
    }

    Ok(parts)
}

// ============================================================================
// Matcher
// ============================================================================

impl PathTagSchema {
    /// Match a relative path (extension pre-stripped) against this schema.
    ///
    /// Returns extracted tag values on match, or a human-readable mismatch description.
    pub fn extract(&self, sub_path: &str) -> PathSchemaMatchResult {
        let actual_segments: Vec<&str> = sub_path.split('/').collect();

        if actual_segments.len() != self.segments.len() {
            return PathSchemaMatchResult::StructureMismatch(format!(
                "expected {} path segment(s), got {}",
                self.segments.len(),
                actual_segments.len()
            ));
        }

        let mut extracted = HashMap::new();

        for (seg_idx, (schema_seg, actual_seg)) in
            self.segments.iter().zip(actual_segments.iter()).enumerate()
        {
            match match_segment(&schema_seg.parts, actual_seg) {
                SegmentMatchResult::Match(tags) => {
                    for (k, v) in tags {
                        extracted.insert(k, v);
                    }
                }
                SegmentMatchResult::Mismatch(desc) => {
                    return PathSchemaMatchResult::StructureMismatch(format!(
                        "segment {}: {}",
                        seg_idx + 1,
                        desc,
                    ));
                }
            }
        }

        PathSchemaMatchResult::Match(extracted)
    }
}

enum SegmentMatchResult {
    Match(Vec<(String, String)>),
    Mismatch(String),
}

/// Match a single segment's parts against an actual segment string.
fn match_segment(parts: &[SchemaPart], actual: &str) -> SegmentMatchResult {
    let mut extracted = Vec::new();
    let mut pos = 0;
    let chars: Vec<char> = actual.chars().collect();

    for (part_idx, part) in parts.iter().enumerate() {
        match part {
            SchemaPart::Literal(lit) => {
                let lit_chars: Vec<char> = lit.chars().collect();
                if pos + lit_chars.len() > chars.len() {
                    return SegmentMatchResult::Mismatch(format!(
                        "expected literal \"{}\" at position {}, but segment is too short",
                        lit, pos
                    ));
                }
                let actual_slice: String = chars[pos..pos + lit_chars.len()].iter().collect();
                if actual_slice != *lit {
                    return SegmentMatchResult::Mismatch(format!(
                        "expected literal \"{}\" at position {}, got \"{}\"",
                        lit, pos, actual_slice
                    ));
                }
                pos += lit_chars.len();
            }
            SchemaPart::RequiredTag(tag_name) => {
                // Consume up to the start of the next literal or end of segment.
                let end = find_tag_end(parts, part_idx, &chars, pos);
                if end <= pos {
                    return SegmentMatchResult::Mismatch(format!(
                        "required tag ${} at position {} has empty value",
                        tag_name, pos
                    ));
                }
                let value: String = chars[pos..end].iter().collect();
                extracted.push((tag_name.clone(), value));
                pos = end;
            }
            SchemaPart::OptionalGroup {
                parts: group_parts,
                tag_name,
            } => {
                // Try to match the group's literal prefix at the current position.
                match try_match_optional_group(group_parts, tag_name, &chars, pos, parts, part_idx)
                {
                    OptionalGroupResult::Matched(tags, new_pos) => {
                        // Backtrack check: verify the next outer part can still match.
                        // If the optional group consumed content that the following
                        // literal needs, skip the group instead.
                        let remaining_ok = match parts.get(part_idx + 1) {
                            Some(SchemaPart::Literal(lit)) => {
                                let lit_chars: Vec<char> = lit.chars().collect();
                                if new_pos + lit_chars.len() <= chars.len() {
                                    let slice: String =
                                        chars[new_pos..new_pos + lit_chars.len()].iter().collect();
                                    slice == *lit
                                } else {
                                    false
                                }
                            }
                            _ => true,
                        };
                        if remaining_ok {
                            extracted.extend(tags);
                            pos = new_pos;
                        }
                        // else: skip the group (backtrack)
                    }
                    OptionalGroupResult::Skipped => {
                        // Group not present, skip entirely.
                    }
                    OptionalGroupResult::Error => {
                        // Group match failed — treat as absent.
                    }
                }
            }
        }
    }

    // Ensure we consumed the entire segment.
    if pos != chars.len() {
        return SegmentMatchResult::Mismatch(format!(
            "trailing content after position {}: \"{}\"",
            pos,
            chars[pos..].iter().collect::<String>()
        ));
    }

    SegmentMatchResult::Match(extracted)
}

/// Find where a required tag's value ends: at the start of the next literal,
/// or at the end of the segment if there's no next literal.
fn find_tag_end(parts: &[SchemaPart], current_idx: usize, chars: &[char], pos: usize) -> usize {
    // Look ahead for the next literal (including inside optional groups).
    if let Some(next_lit) = find_next_literal(parts, current_idx + 1) {
        // Search for this literal in the remaining string.
        let lit_chars: Vec<char> = next_lit.chars().collect();
        if let Some(found) = find_substring(chars, &lit_chars, pos) {
            return found;
        }
    }
    // No next literal found — consume to end of segment.
    chars.len()
}

/// Find the next literal string after the given index in the parts list.
/// Looks into optional groups for their leading literal.
fn find_next_literal(parts: &[SchemaPart], start_idx: usize) -> Option<String> {
    for part in parts.iter().skip(start_idx) {
        match part {
            SchemaPart::Literal(lit) => return Some(lit.clone()),
            SchemaPart::OptionalGroup { .. } => {
                // The optional group might not be present, so also check
                // parts after it. But if the first part of the group is a
                // literal, we can tentatively use it — the optional group
                // matching logic will handle the actual decision.
                //
                // Skip optional groups and look for the next concrete literal.
                continue;
            }
            SchemaPart::RequiredTag(_) => {
                // Two consecutive tags with no literal between them —
                // the first tag consumes everything up to the next literal
                // or end of segment.
                continue;
            }
        }
    }
    None
}

/// Find the first occurrence of `needle` in `haystack` starting at `start`.
fn find_substring(haystack: &[char], needle: &[char], start: usize) -> Option<usize> {
    if needle.is_empty() {
        return Some(start);
    }
    haystack[start..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + start)
}

enum OptionalGroupResult {
    Matched(Vec<(String, String)>, usize),
    Skipped,
    Error,
}

/// Try to match an optional group at the current position.
///
/// Strategy: the group's first part is typically a literal prefix. If the actual
/// path has that literal at the current position, the group is "present" and we
/// consume the whole group. Otherwise, we skip it entirely.
fn try_match_optional_group(
    group_parts: &[SchemaPart],
    _tag_name: &str,
    chars: &[char],
    pos: usize,
    outer_parts: &[SchemaPart],
    outer_idx: usize,
) -> OptionalGroupResult {
    if group_parts.is_empty() {
        return OptionalGroupResult::Skipped;
    }

    // Find the group's leading literal (if any).
    let leading_literal = match &group_parts[0] {
        SchemaPart::Literal(lit) => Some(lit.clone()),
        _ => None,
    };

    if let Some(ref lit) = leading_literal {
        let lit_chars: Vec<char> = lit.chars().collect();
        // Check if the literal matches at the current position.
        if pos + lit_chars.len() <= chars.len() {
            let actual_slice: String = chars[pos..pos + lit_chars.len()].iter().collect();
            if actual_slice == *lit {
                // Group is present — match all parts within.
                let mut group_extracted = Vec::new();
                let mut gpos = pos;

                for (gidx, gpart) in group_parts.iter().enumerate() {
                    match gpart {
                        SchemaPart::Literal(l) => {
                            let l_chars: Vec<char> = l.chars().collect();
                            // Already confirmed the leading literal matches.
                            gpos += l_chars.len();
                        }
                        SchemaPart::RequiredTag(name) => {
                            // Find where this tag ends: next literal in group, or
                            // if last in group, next literal in outer parts.
                            let end = if let Some(next_lit) =
                                find_next_literal_in_group(group_parts, gidx + 1)
                            {
                                let nl_chars: Vec<char> = next_lit.chars().collect();
                                find_substring(chars, &nl_chars, gpos).unwrap_or(chars.len())
                            } else {
                                // Last part in group — look at outer parts for boundary.
                                find_tag_end(outer_parts, outer_idx, chars, gpos)
                            };
                            if end <= gpos {
                                return OptionalGroupResult::Error;
                            }
                            let value: String = chars[gpos..end].iter().collect();
                            group_extracted.push((name.clone(), value));
                            gpos = end;
                        }
                        SchemaPart::OptionalGroup { .. } => {
                            // Nested groups are rejected by the parser.
                            unreachable!("nested optional groups are not allowed");
                        }
                    }
                }

                return OptionalGroupResult::Matched(group_extracted, gpos);
            }
        }

        // Leading literal doesn't match — group is absent.
        return OptionalGroupResult::Skipped;
    }

    // No leading literal — group starts with a tag. This is ambiguous;
    // we can't determine presence without looking ahead. Skip the group.
    // (The parser could warn about this pattern.)
    OptionalGroupResult::Skipped
}

/// Find the next literal within group_parts starting at index.
fn find_next_literal_in_group(group_parts: &[SchemaPart], start_idx: usize) -> Option<String> {
    for part in group_parts.iter().skip(start_idx) {
        if let SchemaPart::Literal(lit) = part {
            return Some(lit.clone());
        }
    }
    None
}

// ============================================================================
// Serde support for PathTagSchema
// ============================================================================

impl serde::Serialize for PathTagSchema {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.template.serialize(serializer)
    }
}

impl<'de> serde::Deserialize<'de> for PathTagSchema {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let template = String::deserialize(deserializer)?;
        parse_path_schema(&template).map_err(serde::de::Error::custom)
    }
}

impl PartialEq for PathTagSchema {
    fn eq(&self, other: &Self) -> bool {
        self.template == other.template
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_tags() {
        let schema = parse_path_schema("$ARTIST/$ALBUM/$TITLE").unwrap();
        assert_eq!(schema.segments.len(), 3);

        // Each segment should have exactly one RequiredTag.
        for (seg, expected) in schema.segments.iter().zip(["ARTIST", "ALBUM", "TITLE"]) {
            assert_eq!(seg.parts.len(), 1);
            match &seg.parts[0] {
                SchemaPart::RequiredTag(name) => assert_eq!(name, expected),
                _ => panic!("expected RequiredTag"),
            }
        }
    }

    #[test]
    fn test_parse_braced_tags() {
        let schema = parse_path_schema("${ARTIST}/${ALBUM}").unwrap();
        assert_eq!(schema.segments.len(), 2);
        match &schema.segments[0].parts[0] {
            SchemaPart::RequiredTag(name) => assert_eq!(name, "ARTIST"),
            _ => panic!("expected RequiredTag"),
        }
    }

    #[test]
    fn test_parse_literals_and_tags() {
        let schema = parse_path_schema("$ARTIST - $ALBUM - $TITLE").unwrap();
        assert_eq!(schema.segments.len(), 1);
        let parts = &schema.segments[0].parts;
        assert_eq!(parts.len(), 5);
        assert!(matches!(&parts[0], SchemaPart::RequiredTag(n) if n == "ARTIST"));
        assert!(matches!(&parts[1], SchemaPart::Literal(s) if s == " - "));
        assert!(matches!(&parts[2], SchemaPart::RequiredTag(n) if n == "ALBUM"));
        assert!(matches!(&parts[3], SchemaPart::Literal(s) if s == " - "));
        assert!(matches!(&parts[4], SchemaPart::RequiredTag(n) if n == "TITLE"));
    }

    #[test]
    fn test_parse_optional_group() {
        let schema = parse_path_schema("$ARTIST$[ - $ALBUM] - $TITLE").unwrap();
        assert_eq!(schema.segments.len(), 1);
        let parts = &schema.segments[0].parts;
        assert_eq!(parts.len(), 4); // RequiredTag, OptionalGroup, Literal, RequiredTag
        assert!(matches!(&parts[0], SchemaPart::RequiredTag(n) if n == "ARTIST"));
        match &parts[1] {
            SchemaPart::OptionalGroup { tag_name, parts } => {
                assert_eq!(tag_name, "ALBUM");
                assert_eq!(parts.len(), 2);
                assert!(matches!(&parts[0], SchemaPart::Literal(s) if s == " - "));
                assert!(matches!(&parts[1], SchemaPart::RequiredTag(n) if n == "ALBUM"));
            }
            _ => panic!("expected OptionalGroup"),
        }
        assert!(matches!(&parts[2], SchemaPart::Literal(s) if s == " - "));
        assert!(matches!(&parts[3], SchemaPart::RequiredTag(n) if n == "TITLE"));
    }

    #[test]
    fn test_parse_error_unterminated_brace() {
        assert!(matches!(
            parse_path_schema("${ARTIST"),
            Err(SchemaParseError::UnterminatedBrace(_))
        ));
    }

    #[test]
    fn test_parse_error_unterminated_optional() {
        assert!(matches!(
            parse_path_schema("$[- $ALBUM"),
            Err(SchemaParseError::UnterminatedOptionalGroup(_))
        ));
    }

    #[test]
    fn test_parse_error_empty_template() {
        assert!(matches!(
            parse_path_schema(""),
            Err(SchemaParseError::EmptyTemplate)
        ));
    }

    #[test]
    fn test_parse_error_no_tag_in_optional() {
        assert!(matches!(
            parse_path_schema("$[literal only]"),
            Err(SchemaParseError::NoTagInOptionalGroup(_))
        ));
    }

    #[test]
    fn test_parse_error_multiple_tags_in_optional() {
        assert!(matches!(
            parse_path_schema("$[$A - $B]"),
            Err(SchemaParseError::MultipleTagsInOptionalGroup(_))
        ));
    }

    #[test]
    fn test_extract_simple() {
        let schema = parse_path_schema("$ARTIST/$ALBUM/$TITLE").unwrap();
        match schema.extract("Pink Floyd/The Wall/Comfortably Numb") {
            PathSchemaMatchResult::Match(tags) => {
                assert_eq!(tags["ARTIST"], "Pink Floyd");
                assert_eq!(tags["ALBUM"], "The Wall");
                assert_eq!(tags["TITLE"], "Comfortably Numb");
            }
            PathSchemaMatchResult::StructureMismatch(desc) => {
                panic!("unexpected mismatch: {}", desc);
            }
        }
    }

    #[test]
    fn test_extract_with_separators() {
        let schema = parse_path_schema("$ARTIST - $ALBUM - $TITLE").unwrap();
        match schema.extract("Artist - Album - Title") {
            PathSchemaMatchResult::Match(tags) => {
                assert_eq!(tags["ARTIST"], "Artist");
                assert_eq!(tags["ALBUM"], "Album");
                assert_eq!(tags["TITLE"], "Title");
            }
            PathSchemaMatchResult::StructureMismatch(desc) => {
                panic!("unexpected mismatch: {}", desc);
            }
        }
    }

    #[test]
    fn test_extract_with_optional_present() {
        let schema = parse_path_schema("$ARTIST$[ - $ALBUM] - $TITLE").unwrap();
        match schema.extract("Artist - Album - Title") {
            PathSchemaMatchResult::Match(tags) => {
                assert_eq!(tags["ARTIST"], "Artist");
                assert_eq!(tags["ALBUM"], "Album");
                assert_eq!(tags["TITLE"], "Title");
            }
            PathSchemaMatchResult::StructureMismatch(desc) => {
                panic!("unexpected mismatch: {}", desc);
            }
        }
    }

    #[test]
    fn test_extract_with_optional_absent() {
        let schema = parse_path_schema("$ARTIST$[ - $ALBUM] - $TITLE").unwrap();
        // When the optional group is absent, the path is just "Artist - Title"
        // The `$ARTIST` tag consumes "Artist", then `$[ - $ALBUM]` is skipped
        // (because the next thing is " - " which is also the trailing literal),
        // then " - " literal matches, then $TITLE = "Title".
        match schema.extract("Artist - Title") {
            PathSchemaMatchResult::Match(tags) => {
                assert_eq!(tags["ARTIST"], "Artist");
                assert!(!tags.contains_key("ALBUM"));
                assert_eq!(tags["TITLE"], "Title");
            }
            PathSchemaMatchResult::StructureMismatch(desc) => {
                panic!("unexpected mismatch: {}", desc);
            }
        }
    }

    #[test]
    fn test_extract_full_example() {
        let schema =
            parse_path_schema("$LABEL/$CATALOGNUMBER/$ARTIST$[ - $ALBUM] - $TITLE").unwrap();
        match schema.extract("monstercat/MCEP123/Artist - Album - Title") {
            PathSchemaMatchResult::Match(tags) => {
                assert_eq!(tags["LABEL"], "monstercat");
                assert_eq!(tags["CATALOGNUMBER"], "MCEP123");
                assert_eq!(tags["ARTIST"], "Artist");
                assert_eq!(tags["ALBUM"], "Album");
                assert_eq!(tags["TITLE"], "Title");
            }
            PathSchemaMatchResult::StructureMismatch(desc) => {
                panic!("unexpected mismatch: {}", desc);
            }
        }
    }

    #[test]
    fn test_extract_segment_count_mismatch() {
        let schema = parse_path_schema("$ARTIST/$ALBUM/$TITLE").unwrap();
        match schema.extract("Artist/Album") {
            PathSchemaMatchResult::StructureMismatch(desc) => {
                assert!(desc.contains("expected 3 path segment(s), got 2"));
            }
            PathSchemaMatchResult::Match(_) => panic!("expected mismatch"),
        }
    }

    #[test]
    fn test_extract_literal_mismatch() {
        let schema = parse_path_schema("$ARTIST - $TITLE").unwrap();
        match schema.extract("ArtistXTitle") {
            PathSchemaMatchResult::StructureMismatch(desc) => {
                assert!(desc.contains("expected literal"));
            }
            PathSchemaMatchResult::Match(_) => panic!("expected mismatch"),
        }
    }

    #[test]
    fn test_extract_trailing_content() {
        let schema = parse_path_schema("$ARTIST").unwrap();
        // Single tag in single segment — should consume everything.
        match schema.extract("Artist Name") {
            PathSchemaMatchResult::Match(tags) => {
                assert_eq!(tags["ARTIST"], "Artist Name");
            }
            PathSchemaMatchResult::StructureMismatch(desc) => {
                panic!("unexpected mismatch: {}", desc);
            }
        }
    }

    #[test]
    fn test_extract_leading_literal() {
        let schema = parse_path_schema("Release - $TITLE").unwrap();
        match schema.extract("Release - My Song") {
            PathSchemaMatchResult::Match(tags) => {
                assert_eq!(tags["TITLE"], "My Song");
            }
            PathSchemaMatchResult::StructureMismatch(desc) => {
                panic!("unexpected mismatch: {}", desc);
            }
        }
    }

    #[test]
    fn test_tag_name_case_insensitive_in_parse() {
        let schema = parse_path_schema("$artist/$Album").unwrap();
        match &schema.segments[0].parts[0] {
            SchemaPart::RequiredTag(name) => assert_eq!(name, "ARTIST"),
            _ => panic!("expected RequiredTag"),
        }
        match &schema.segments[1].parts[0] {
            SchemaPart::RequiredTag(name) => assert_eq!(name, "ALBUM"),
            _ => panic!("expected RequiredTag"),
        }
    }

    #[test]
    fn test_serde_round_trip() {
        let schema = parse_path_schema("$LABEL/$ARTIST - $TITLE").unwrap();
        let encoded = bincode::serialize(&schema).unwrap();
        let deserialized: PathTagSchema = bincode::deserialize(&encoded).unwrap();
        assert_eq!(schema.template, deserialized.template);
        assert_eq!(schema.segments.len(), deserialized.segments.len());
    }

    #[test]
    fn test_partial_eq() {
        let a = parse_path_schema("$A/$B").unwrap();
        let b = parse_path_schema("$A/$B").unwrap();
        let c = parse_path_schema("$X/$Y").unwrap();
        assert_eq!(a, b);
        assert_ne!(a, c);
    }
}
