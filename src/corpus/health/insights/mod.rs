//! Computed Insights over Health Signals
//!
//! Insights are real-time computed views that aggregate signals into
//! actionable recommendations. They are never stored - always fresh.
//!
//! ## One-Dimensional Insights
//!
//! Single signal type aggregation. Cheap, immediate computation.
//! Examples: deployment conflicts count, missing tags count
//!
//! ## Multi-Dimensional Insights
//!
//! TODO: Multi-dim insights will be reimplemented via TaskDaemon Computations.
//! Currently stubbed out.

mod one_dim;


// =============================================================================
// Multi-Dim Stubs (to be reimplemented via TaskDaemon)
// =============================================================================

/// Types of multi-dimensional insights (stub).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiDimInsightType {
    /// Fingerprint duplicates with quality comparison
    QualityDuplicates,
}

/// Handle for tracking a multi-dim computation (stub).
pub struct InsightHandle {
    _private: (),
}

impl InsightHandle {
    /// Try to receive a message (always returns None - stub).
    pub fn try_recv(&self) -> Option<InsightMessage> {
        None
    }
}

/// Messages from multi-dim computation (stub).
#[derive(Debug, Clone)]
pub enum InsightMessage {
    /// Progress update
    Progress(f32),
    /// Computation complete
    Complete(Insight),
    /// Computation failed
    Error(String),
}

/// Spawn a multi-dim insight computation (stub - does nothing).
pub fn spawn_multi_dim_insight(_db_path: &str, _insight_type: MultiDimInsightType) -> InsightHandle {
    InsightHandle { _private: () }
}

/// Stub - flows don't exist yet.
///
/// When flows are implemented, this will enumerate the available
/// resolution flows that can be launched from insights.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowType {
    /// Placeholder until flows are implemented
    Placeholder,
}

/// Severity level for insights (UI-agnostic).
///
/// The UI layer maps this to HealthStatus for consistent coloring.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum InsightSeverity {
    /// Everything is good - reassuring information
    Healthy,
    /// Informational - opportunities for improvement
    Info,
    /// Warning - attention may be needed
    Warning,
    /// Critical - action required
    Critical,
}

/// A computed insight over health signals.
///
/// Insights are never stored - they are computed fresh from the current
/// signal state. They aggregate signals into actionable recommendations.
#[derive(Debug, Clone)]
pub enum Insight {
    // =========================================================================
    // Multi-Dimensional (background computed, lift to top when ready)
    // =========================================================================
    /// Fingerprint duplicates where quality comparison is possible.
    ///
    /// Quality is introspected from Track metadata (file_type, bitrate_kbps) at runtime.
    QualityDuplicates {
        /// Number of duplicate groups
        dupe_groups: usize,
        /// Total tracks involved
        total_tracks: usize,
        /// Groups with a clear quality winner (e.g., FLAC vs MP3)
        clear_winner: usize,
    },

    // =========================================================================
    // One-Dimensional (immediate computation)
    // =========================================================================
    /// Multiple corpus files would deploy to the same library path.
    DeploymentConflicts {
        /// Number of conflicting paths
        count: usize,
    },

    /// Tag value variants needing canonicalization.
    TagCanonicalization {
        /// Which tag field (artist, album_artist, genre, album)
        field: String,
        /// Number of variant spellings
        variant_count: usize,
        /// Tracks affected by variants
        affected_tracks: usize,
    },

    /// External changes detected (files modified outside MLA).
    OutOfBandChanges {
        /// Tags on disk differ from index
        tag_changes: usize,
        /// Files replaced or modified
        file_changes: usize,
    },

    /// Index and filesystem are out of sync.
    IndexDesync {
        /// Indexed files no longer on disk
        missing_from_disk: usize,
        /// Files on disk not in index
        missing_from_index: usize,
        /// Files moved to new location (same inode)
        relocated: usize,
    },

    /// Required tags missing from tracks.
    MissingTags {
        /// Which tag is missing
        tag_name: String,
        /// Number of tracks missing this tag
        count: usize,
    },

    // =========================================================================
    // Reassuring Health (always shown at bottom)
    // =========================================================================
    /// Overall corpus health summary.
    CorpusHealth {
        /// Total tracks in index
        total_tracks: usize,
        /// Index matches filesystem
        indexed_healthy: bool,
        /// All libraries are healthy
        libraries_healthy: bool,
        /// DB tags match disk tags
        tags_synced: bool,
    },

    // =========================================================================
    // Computing Placeholder
    // =========================================================================
    /// Placeholder while a multi-dim insight is being computed.
    Computing {
        /// Which insight type is being computed
        insight_type: MultiDimInsightType,
        /// Progress (0.0 to 1.0), if available
        progress: Option<f32>,
    },
}

impl Insight {
    /// Priority for display ordering (higher = more prominent).
    ///
    /// Multi-dimensional insights are lifted to top because they
    /// synthesize more information and are more actionable.
    pub fn priority(&self) -> u32 {
        match self {
            // Multi-dim at top (most actionable)
            Insight::QualityDuplicates { .. } => 100,

            // One-dim by severity
            Insight::IndexDesync { .. } => 80,
            Insight::DeploymentConflicts { .. } => 75,
            Insight::OutOfBandChanges { .. } => 70,
            Insight::TagCanonicalization { .. } => 50,
            Insight::MissingTags { .. } => 40,

            // Reassuring at bottom
            Insight::CorpusHealth { .. } => 10,

            // Computing shows in-place based on target type priority
            Insight::Computing { insight_type, .. } => match insight_type {
                MultiDimInsightType::QualityDuplicates => 100,
            },
        }
    }

    /// Severity level for UI coloring.
    pub fn severity(&self) -> InsightSeverity {
        match self {
            Insight::CorpusHealth {
                indexed_healthy,
                libraries_healthy,
                tags_synced,
                ..
            } => {
                if *indexed_healthy && *libraries_healthy && *tags_synced {
                    InsightSeverity::Healthy
                } else {
                    InsightSeverity::Warning
                }
            }

            Insight::IndexDesync {
                missing_from_disk, ..
            } if *missing_from_disk > 0 => InsightSeverity::Critical,

            Insight::OutOfBandChanges {
                file_changes,
                tag_changes,
            } if *file_changes > 0 || *tag_changes > 0 => InsightSeverity::Warning,

            Insight::DeploymentConflicts { count } if *count > 0 => InsightSeverity::Warning,

            Insight::QualityDuplicates { dupe_groups, .. } if *dupe_groups > 0 => {
                InsightSeverity::Info
            }

            Insight::TagCanonicalization { variant_count, .. } if *variant_count > 0 => {
                InsightSeverity::Info
            }

            Insight::MissingTags { count, .. } if *count > 0 => InsightSeverity::Info,

            Insight::Computing { .. } => InsightSeverity::Info,

            _ => InsightSeverity::Healthy,
        }
    }

    /// Whether this insight is actionable (would launch a flow).
    ///
    /// Returns false for purely informational insights and computing placeholders.
    pub fn is_actionable(&self) -> bool {
        match self {
            Insight::CorpusHealth { .. } => false,
            Insight::Computing { .. } => false,
            _ => self.item_count() > 0,
        }
    }

    /// Number of items this insight represents.
    ///
    /// Used for prioritization and display (e.g., "5 conflicts").
    pub fn item_count(&self) -> usize {
        match self {
            Insight::QualityDuplicates { dupe_groups, .. } => *dupe_groups,
            Insight::DeploymentConflicts { count } => *count,
            Insight::TagCanonicalization { variant_count, .. } => *variant_count,
            Insight::OutOfBandChanges {
                tag_changes,
                file_changes,
            } => tag_changes + file_changes,
            Insight::IndexDesync {
                missing_from_disk,
                missing_from_index,
                relocated,
            } => missing_from_disk + missing_from_index + relocated,
            Insight::MissingTags { count, .. } => *count,
            Insight::CorpusHealth { .. } => 0,
            Insight::Computing { .. } => 0,
        }
    }

    /// Which flow would resolve this insight (stub).
    ///
    /// Returns `FlowType::Placeholder` until flows are implemented.
    pub fn flow_type(&self) -> FlowType {
        FlowType::Placeholder
    }

    /// Display label for the insight.
    pub fn label(&self) -> String {
        match self {
            Insight::QualityDuplicates {
                dupe_groups,
                clear_winner,
                ..
            } => {
                format!(
                    "Quality Duplicates: {} groups ({} with clear winner)",
                    dupe_groups, clear_winner
                )
            }

            Insight::DeploymentConflicts { count } => {
                format!("Deployment Conflicts: {}", count)
            }

            Insight::TagCanonicalization {
                field,
                variant_count,
                affected_tracks,
            } => {
                format!(
                    "{} canonicalization: {} variants ({} tracks)",
                    field, variant_count, affected_tracks
                )
            }

            Insight::OutOfBandChanges {
                tag_changes,
                file_changes,
            } => {
                format!(
                    "External changes: {} tag edits, {} file changes",
                    tag_changes, file_changes
                )
            }

            Insight::IndexDesync {
                missing_from_disk,
                missing_from_index,
                relocated,
            } => {
                format!(
                    "Index sync: {} missing, {} new, {} moved",
                    missing_from_disk, missing_from_index, relocated
                )
            }

            Insight::MissingTags { tag_name, count } => {
                format!("Missing {}: {} tracks", tag_name, count)
            }

            Insight::CorpusHealth { total_tracks, .. } => {
                format!("Corpus: {} tracks indexed", total_tracks)
            }

            Insight::Computing {
                insight_type,
                progress,
            } => {
                let pct = progress
                    .map(|p| format!(" ({:.0}%)", p * 100.0))
                    .unwrap_or_default();
                match insight_type {
                    MultiDimInsightType::QualityDuplicates => {
                        format!("Computing quality duplicates...{}", pct)
                    }
                }
            }
        }
    }

    /// Short description of what resolving this insight does.
    pub fn description(&self) -> &'static str {
        match self {
            Insight::QualityDuplicates { .. } => {
                "Remove lower-quality duplicates when higher-quality versions exist"
            }
            Insight::DeploymentConflicts { .. } => {
                "Resolve files that would deploy to the same library path"
            }
            Insight::TagCanonicalization { .. } => {
                "Unify variant spellings to a canonical form"
            }
            Insight::OutOfBandChanges { .. } => {
                "Sync index with external file/tag changes"
            }
            Insight::IndexDesync { .. } => "Reconcile index with filesystem state",
            Insight::MissingTags { .. } => "Add missing required tags to tracks",
            Insight::CorpusHealth { .. } => "Overall corpus health summary",
            Insight::Computing { .. } => "Computation in progress...",
        }
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insight_priority_ordering() {
        // Multi-dim should have highest priority
        let quality = Insight::QualityDuplicates {
            dupe_groups: 10,
            total_tracks: 20,
            clear_winner: 5,
        };
        let health = Insight::CorpusHealth {
            total_tracks: 1000,
            indexed_healthy: true,
            libraries_healthy: true,
            tags_synced: true,
        };

        assert!(quality.priority() > health.priority());
    }

    #[test]
    fn test_insight_severity() {
        let healthy = Insight::CorpusHealth {
            total_tracks: 1000,
            indexed_healthy: true,
            libraries_healthy: true,
            tags_synced: true,
        };
        assert_eq!(healthy.severity(), InsightSeverity::Healthy);

        let critical = Insight::IndexDesync {
            missing_from_disk: 5,
            missing_from_index: 0,
            relocated: 0,
        };
        assert_eq!(critical.severity(), InsightSeverity::Critical);
    }

    #[test]
    fn test_insight_actionable() {
        let actionable = Insight::DeploymentConflicts { count: 3 };
        assert!(actionable.is_actionable());

        let not_actionable = Insight::DeploymentConflicts { count: 0 };
        assert!(!not_actionable.is_actionable());

        let health = Insight::CorpusHealth {
            total_tracks: 1000,
            indexed_healthy: true,
            libraries_healthy: true,
            tags_synced: true,
        };
        assert!(!health.is_actionable());
    }
}
