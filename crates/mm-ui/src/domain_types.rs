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
    LaunchOobTagSync,
    LaunchOobTagConflict,
    LaunchMovedFileAcknowledge,
    LaunchCorruptFileResolution,
    LaunchShitFormatTranscode,
    LaunchIntakeConfirmation,
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
// InboxInsightAction
// ============================================================================

/// What action an inbox bucket entry triggers on Enter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InboxInsightAction {
    LaunchIntake,
    LaunchCorpusMatchResolution,
    LaunchInboxTagCanonicity,
    LaunchOrganize,
    LaunchInboxCompoundSplit,
    /// Informational only, no action
    Informational,
}
