//! Domain types shared between TUI and web clients.
//!
//! These types encode routing decisions, tab identities, and action
//! discriminants that both backends need. Moved here from mm-tui so the
//! web client can use them without depending on ratatui.

// ============================================================================
// DeployTab
// ============================================================================

/// The active tab in the deploy signal view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DeployTab {
    #[default]
    Healthy,
    New,
    Conflicts,
    Leftover,
    Stale,
}

impl DeployTab {
    /// Display label for this tab.
    pub fn label(&self) -> &'static str {
        match self {
            DeployTab::Healthy => "Healthy",
            DeployTab::New => "New",
            DeployTab::Conflicts => "Conflicts",
            DeployTab::Leftover => "Leftover",
            DeployTab::Stale => "Stale",
        }
    }

    /// Get the next tab (Right arrow).
    pub fn next(&self) -> Self {
        match self {
            DeployTab::Healthy => DeployTab::New,
            DeployTab::New => DeployTab::Conflicts,
            DeployTab::Conflicts => DeployTab::Leftover,
            DeployTab::Leftover => DeployTab::Stale,
            DeployTab::Stale => DeployTab::Healthy,
        }
    }

    /// Get the previous tab (Left arrow).
    pub fn prev(&self) -> Self {
        match self {
            DeployTab::Healthy => DeployTab::Stale,
            DeployTab::New => DeployTab::Healthy,
            DeployTab::Conflicts => DeployTab::New,
            DeployTab::Leftover => DeployTab::Conflicts,
            DeployTab::Stale => DeployTab::Leftover,
        }
    }

    /// All tabs in order.
    pub fn all() -> &'static [DeployTab] {
        &[
            DeployTab::Healthy,
            DeployTab::New,
            DeployTab::Conflicts,
            DeployTab::Leftover,
            DeployTab::Stale,
        ]
    }

    /// Get the index of this tab (for scroll position array).
    pub fn index(&self) -> usize {
        match self {
            DeployTab::Healthy => 0,
            DeployTab::New => 1,
            DeployTab::Conflicts => 2,
            DeployTab::Leftover => 3,
            DeployTab::Stale => 4,
        }
    }

    /// Parse from URL string.
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "healthy" => Some(Self::Healthy),
            "new" => Some(Self::New),
            "conflicts" => Some(Self::Conflicts),
            "leftover" => Some(Self::Leftover),
            "stale" => Some(Self::Stale),
            _ => None,
        }
    }

    /// Serialize to URL string.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Healthy => "healthy",
            Self::New => "new",
            Self::Conflicts => "conflicts",
            Self::Leftover => "leftover",
            Self::Stale => "stale",
        }
    }
}

// ============================================================================
// InsightAction
// ============================================================================

/// Actions that can be launched from specific insight types.
///
/// Drives routing: which resolution modal to open when the operator
/// confirms an insight entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsightAction {
    LaunchMissingFileResolution,
    LaunchMissingDirectoryResolution,
    LaunchTagCanonicityResolution,
    LaunchCompoundTagSplitSafe,
    LaunchCompoundTagSplitReview,
    LaunchOobResolution,
    LaunchMovedFileAcknowledge,
    LaunchCorruptFileResolution,
    LaunchLosslessRemuxResolution,
    LaunchCrossSourceOverlapResolution,
    LaunchReleaseOverlapResolution,
    LaunchSubparDuplicateResolution,
    LaunchManualReview(mm_meta::views::review_match::ReviewKind),
    LaunchMissingTagResolution,
    LaunchMissingAlbumSingleResolution,
    LaunchDiscExtractionResolution,
    LaunchPathTagMismatchResolution,
    /// Not yet implemented
    NotImplemented,
    /// Informational only - no action available
    Informational,
}

// ============================================================================
// Search types
// ============================================================================

/// Type of search condition.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ConditionType {
    #[default]
    Tag,
    FileType,
    SampleRate,
    Bitrate,
    Duration,
}

impl ConditionType {
    pub fn label(&self) -> &'static str {
        match self {
            ConditionType::Tag => "TAG",
            ConditionType::FileType => "TYPE",
            ConditionType::SampleRate => "RATE",
            ConditionType::Bitrate => "KBPS",
            ConditionType::Duration => "TIME",
        }
    }

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

/// Specific file format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
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

/// File type category for filtering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum FileTypeCategory {
    #[default]
    Any,
    Lossless,
    Lossy,
    Specific(FileFormat),
}

impl FileTypeCategory {
    pub fn label(&self) -> &'static str {
        match self {
            FileTypeCategory::Any => "Any",
            FileTypeCategory::Lossless => "Lossless",
            FileTypeCategory::Lossy => "Lossy",
            FileTypeCategory::Specific(f) => f.label(),
        }
    }

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

    pub fn matches(&self, file_type: &str) -> bool {
        let file_type_lower = file_type.to_lowercase();
        match self {
            FileTypeCategory::Any => true,
            FileTypeCategory::Lossless => {
                matches!(
                    file_type_lower.as_str(),
                    "flac" | "wav" | "alac" | "aiff" | "ape"
                )
            }
            FileTypeCategory::Lossy => {
                matches!(
                    file_type_lower.as_str(),
                    "mp3" | "opus" | "ogg" | "aac" | "m4a"
                )
            }
            FileTypeCategory::Specific(f) => f.matches(&file_type_lower),
        }
    }
}

/// Logical operators for combining search conditions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum LogicalOperator {
    #[default]
    And,
    Or,
    Xor,
}

impl LogicalOperator {
    pub fn label(&self) -> &'static str {
        match self {
            LogicalOperator::And => "AND",
            LogicalOperator::Or => "OR",
            LogicalOperator::Xor => "XOR",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            LogicalOperator::And => LogicalOperator::Or,
            LogicalOperator::Or => LogicalOperator::Xor,
            LogicalOperator::Xor => LogicalOperator::And,
        }
    }
}

/// Comparison operators for tag value matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ComparisonOperator {
    #[default]
    Is,
    Not,
    Contains,
    Like,
}

impl ComparisonOperator {
    pub fn label(&self) -> &'static str {
        match self {
            ComparisonOperator::Is => "IS",
            ComparisonOperator::Not => "NOT",
            ComparisonOperator::Contains => "CONTAINS",
            ComparisonOperator::Like => "LIKE",
        }
    }

    pub fn next(&self) -> Self {
        match self {
            ComparisonOperator::Is => ComparisonOperator::Not,
            ComparisonOperator::Not => ComparisonOperator::Contains,
            ComparisonOperator::Contains => ComparisonOperator::Like,
            ComparisonOperator::Like => ComparisonOperator::Is,
        }
    }
}

/// A single search condition.
#[derive(Debug, Clone, Default)]
pub struct SearchCondition {
    pub condition_type: ConditionType,
    pub operator: LogicalOperator,
    pub tag_name: crate::text_input::TextInputState,
    pub comparison: ComparisonOperator,
    pub search_value: crate::text_input::TextInputState,
    pub file_type_category: FileTypeCategory,
    pub range_min: crate::text_input::TextInputState,
    pub range_max: crate::text_input::TextInputState,
}

impl SearchCondition {
    pub fn new() -> Self {
        Self::default()
    }

    /// Extract a wire-safe projection (replaces TextInputState with String,
    /// converts enums to wire mirrors).
    pub fn to_wire(&self) -> mm_meta::domain_query_types::SearchConditionWire {
        use mm_meta::domain_query_types::*;

        let wire_ct = match self.condition_type {
            ConditionType::Tag => WireConditionType::Tag,
            ConditionType::FileType => WireConditionType::FileType,
            ConditionType::SampleRate => WireConditionType::SampleRate,
            ConditionType::Bitrate => WireConditionType::Bitrate,
            ConditionType::Duration => WireConditionType::Duration,
        };
        let wire_op = match self.operator {
            LogicalOperator::And => WireLogicalOperator::And,
            LogicalOperator::Or => WireLogicalOperator::Or,
            LogicalOperator::Xor => WireLogicalOperator::Xor,
        };
        let wire_cmp = match self.comparison {
            ComparisonOperator::Is => WireComparisonOperator::Is,
            ComparisonOperator::Not => WireComparisonOperator::Not,
            ComparisonOperator::Contains => WireComparisonOperator::Contains,
            ComparisonOperator::Like => WireComparisonOperator::Like,
        };
        let wire_ftc = match self.file_type_category {
            FileTypeCategory::Any => WireFileTypeCategory::Any,
            FileTypeCategory::Lossless => WireFileTypeCategory::Lossless,
            FileTypeCategory::Lossy => WireFileTypeCategory::Lossy,
            FileTypeCategory::Specific(FileFormat::Flac) => WireFileTypeCategory::Flac,
            FileTypeCategory::Specific(FileFormat::Mp3) => WireFileTypeCategory::Mp3,
            FileTypeCategory::Specific(FileFormat::Opus) => WireFileTypeCategory::Opus,
            FileTypeCategory::Specific(FileFormat::Ogg) => WireFileTypeCategory::Ogg,
            FileTypeCategory::Specific(FileFormat::Wav) => WireFileTypeCategory::Wav,
            FileTypeCategory::Specific(FileFormat::Aac) => WireFileTypeCategory::Aac,
        };

        SearchConditionWire {
            condition_type: wire_ct,
            operator: wire_op,
            tag_name: self.tag_name.value().to_string(),
            comparison: wire_cmp,
            search_value: self.search_value.value().to_string(),
            file_type_category: wire_ftc,
            range_min: self.range_min.value().to_string(),
            range_max: self.range_max.value().to_string(),
        }
    }
}

/// Current mode of the tag search view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum TagSearchMode {
    #[default]
    QueryBuilder,
    Results,
}

/// Searchable tag field names.
pub const SEARCHABLE_TAGS: &[&str] = &["artist", "album", "album_artist", "title", "genre"];

