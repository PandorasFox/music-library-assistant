//! Tag Search Types
//!
//! Types for the tag search query builder and results.
//!
//! Supports both tag-based conditions and file metadata conditions.

use crate::corpus::db::types::AudioFile;

// ============================================================================
// Condition Type
// ============================================================================

/// Type of search condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConditionType {
    /// Tag-based condition (existing behavior)
    #[default]
    Tag,
    /// File type filter (lossless/lossy, specific formats)
    FileType,
    /// Sample rate range filter
    SampleRate,
    /// Bitrate range filter
    Bitrate,
    /// Duration range filter
    Duration,
}

impl ConditionType {
    /// Display label for the condition type.
    pub fn label(&self) -> &'static str {
        match self {
            ConditionType::Tag => "TAG",
            ConditionType::FileType => "TYPE",
            ConditionType::SampleRate => "RATE",
            ConditionType::Bitrate => "KBPS",
            ConditionType::Duration => "TIME",
        }
    }

    /// Cycle to the next condition type.
    pub fn next(&self) -> Self {
        match self {
            ConditionType::Tag => ConditionType::FileType,
            ConditionType::FileType => ConditionType::SampleRate,
            ConditionType::SampleRate => ConditionType::Bitrate,
            ConditionType::Bitrate => ConditionType::Duration,
            ConditionType::Duration => ConditionType::Tag,
        }
    }

}

// ============================================================================
// File Type Categories
// ============================================================================

/// File type category for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileTypeCategory {
    /// Any file type (no filter)
    #[default]
    Any,
    /// Lossless formats (flac, wav, alac, aiff)
    Lossless,
    /// Lossy formats (mp3, opus, ogg, aac)
    Lossy,
    /// Specific format
    Specific(FileFormat),
}

impl FileTypeCategory {
    /// Display label for the category.
    pub fn label(&self) -> &'static str {
        match self {
            FileTypeCategory::Any => "Any",
            FileTypeCategory::Lossless => "Lossless",
            FileTypeCategory::Lossy => "Lossy",
            FileTypeCategory::Specific(f) => f.label(),
        }
    }

    /// Cycle to the next category.
    pub fn next(&self) -> Self {
        match self {
            FileTypeCategory::Any => FileTypeCategory::Lossless,
            FileTypeCategory::Lossless => FileTypeCategory::Lossy,
            FileTypeCategory::Lossy => FileTypeCategory::Specific(FileFormat::Flac),
            FileTypeCategory::Specific(f) => {
                let next_format = f.next();
                if next_format == FileFormat::Flac {
                    FileTypeCategory::Any
                } else {
                    FileTypeCategory::Specific(next_format)
                }
            }
        }
    }

    /// Cycle to the previous category.
    pub fn prev(&self) -> Self {
        match self {
            FileTypeCategory::Any => FileTypeCategory::Specific(FileFormat::Aac),
            FileTypeCategory::Lossless => FileTypeCategory::Any,
            FileTypeCategory::Lossy => FileTypeCategory::Lossless,
            FileTypeCategory::Specific(f) => {
                let prev_format = f.prev();
                if prev_format == FileFormat::Aac {
                    FileTypeCategory::Lossy
                } else {
                    FileTypeCategory::Specific(prev_format)
                }
            }
        }
    }

    /// Check if a file type matches this category.
    pub fn matches(&self, file_type: &str) -> bool {
        let file_type_lower = file_type.to_lowercase();
        match self {
            FileTypeCategory::Any => true,
            FileTypeCategory::Lossless => {
                matches!(file_type_lower.as_str(), "flac" | "wav" | "alac" | "aiff" | "ape")
            }
            FileTypeCategory::Lossy => {
                matches!(file_type_lower.as_str(), "mp3" | "opus" | "ogg" | "aac" | "m4a")
            }
            FileTypeCategory::Specific(f) => f.matches(&file_type_lower),
        }
    }
}

/// Specific file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FileFormat {
    #[default]
    Flac,
    Mp3,
    Opus,
    Ogg,
    Wav,
    Aac,
}

impl FileFormat {
    /// Display label for the format.
    pub fn label(&self) -> &'static str {
        match self {
            FileFormat::Flac => "FLAC",
            FileFormat::Mp3 => "MP3",
            FileFormat::Opus => "Opus",
            FileFormat::Ogg => "OGG",
            FileFormat::Wav => "WAV",
            FileFormat::Aac => "AAC",
        }
    }

    /// Cycle to the next format.
    pub fn next(&self) -> Self {
        match self {
            FileFormat::Flac => FileFormat::Mp3,
            FileFormat::Mp3 => FileFormat::Opus,
            FileFormat::Opus => FileFormat::Ogg,
            FileFormat::Ogg => FileFormat::Wav,
            FileFormat::Wav => FileFormat::Aac,
            FileFormat::Aac => FileFormat::Flac,
        }
    }

    /// Cycle to the previous format.
    pub fn prev(&self) -> Self {
        match self {
            FileFormat::Flac => FileFormat::Aac,
            FileFormat::Mp3 => FileFormat::Flac,
            FileFormat::Opus => FileFormat::Mp3,
            FileFormat::Ogg => FileFormat::Opus,
            FileFormat::Wav => FileFormat::Ogg,
            FileFormat::Aac => FileFormat::Wav,
        }
    }

    /// Check if a file type matches this format.
    pub fn matches(&self, file_type: &str) -> bool {
        match self {
            FileFormat::Flac => file_type == "flac",
            FileFormat::Mp3 => file_type == "mp3",
            FileFormat::Opus => file_type == "opus",
            FileFormat::Ogg => file_type == "ogg",
            FileFormat::Wav => file_type == "wav",
            FileFormat::Aac => file_type == "aac" || file_type == "m4a",
        }
    }
}

// ============================================================================
// Logical operators for combining search conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogicalOperator {
    #[default]
    And,
    Or,
    Xor,
}

impl LogicalOperator {
    /// Display label for the operator.
    pub fn label(&self) -> &'static str {
        match self {
            LogicalOperator::And => "AND",
            LogicalOperator::Or => "OR",
            LogicalOperator::Xor => "XOR",
        }
    }

    /// Cycle to the next operator.
    pub fn next(&self) -> Self {
        match self {
            LogicalOperator::And => LogicalOperator::Or,
            LogicalOperator::Or => LogicalOperator::Xor,
            LogicalOperator::Xor => LogicalOperator::And,
        }
    }

}

/// Comparison operators for tag value matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ComparisonOperator {
    /// Exact match (case-insensitive) - default
    #[default]
    Is,
    /// Negated exact match
    Not,
    /// Substring match (case-insensitive)
    Contains,
    /// SQL LIKE pattern (% and _ wildcards)
    Like,
}

impl ComparisonOperator {
    /// Display label for the operator.
    pub fn label(&self) -> &'static str {
        match self {
            ComparisonOperator::Is => "IS",
            ComparisonOperator::Not => "NOT",
            ComparisonOperator::Contains => "CONTAINS",
            ComparisonOperator::Like => "LIKE",
        }
    }

    /// Cycle to the next operator.
    pub fn next(&self) -> Self {
        match self {
            ComparisonOperator::Is => ComparisonOperator::Not,
            ComparisonOperator::Not => ComparisonOperator::Contains,
            ComparisonOperator::Contains => ComparisonOperator::Like,
            ComparisonOperator::Like => ComparisonOperator::Is,
        }
    }

    /// Cycle to the previous operator.
    pub fn prev(&self) -> Self {
        match self {
            ComparisonOperator::Is => ComparisonOperator::Like,
            ComparisonOperator::Not => ComparisonOperator::Is,
            ComparisonOperator::Contains => ComparisonOperator::Not,
            ComparisonOperator::Like => ComparisonOperator::Contains,
        }
    }
}

/// A single search condition.
///
/// For Tag conditions: uses tag_name, comparison, and value.
/// For FileType conditions: uses file_type_category.
/// For range conditions (SampleRate, Bitrate, Duration): uses range_min and range_max.
#[derive(Debug, Clone, Default)]
pub struct SearchCondition {
    /// Type of condition (Tag, FileType, SampleRate, etc.)
    pub condition_type: ConditionType,
    /// Logical operator connecting to previous condition (ignored for first).
    pub operator: LogicalOperator,
    /// Tag name to search (for Tag conditions).
    pub tag_name: String,
    /// How to compare the tag value (for Tag conditions).
    pub comparison: ComparisonOperator,
    /// Value to search for (for Tag conditions).
    pub value: String,
    /// File type category (for FileType conditions).
    pub file_type_category: FileTypeCategory,
    /// Range minimum (for SampleRate, Bitrate, Duration conditions).
    /// For SampleRate: value in Hz (e.g., "44100")
    /// For Bitrate: value in kbps (e.g., "320")
    /// For Duration: value in seconds (e.g., "180")
    pub range_min: String,
    /// Range maximum (for SampleRate, Bitrate, Duration conditions).
    pub range_max: String,
}

impl SearchCondition {
    pub fn new() -> Self {
        Self::default()
    }
}

/// Current mode of the tag search view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TagSearchMode {
    #[default]
    QueryBuilder,
    Results,
}

/// Actions returned from tag search key handling.
#[derive(Debug, Clone)]
pub enum TagSearchAction {
    /// No action needed.
    None,
    /// Cancel tag search (Escape when nothing to cancel).
    Cancel,
    /// Cycle to next lateral view (Tab).
    CycleNext,
    /// Cycle to previous lateral view (Shift-Tab).
    CyclePrev,
    /// Execute the search query (requires db access).
    ExecuteSearch,
    /// Open tag editor for a single audio file.
    EditAudioFile(AudioFile),
}

/// Modal dialogs for tag search.
#[derive(Debug, Clone)]
pub enum TagSearchModal {
    /// No results found after search
    NoResults,
    /// Gathering tags for bulk edit (shown briefly before editor opens)
    GatheringTags,
}

/// Searchable tag field names.
pub const SEARCHABLE_TAGS: &[&str] = &[
    "artist",
    "album",
    "album_artist",
    "title",
    "genre",
];
