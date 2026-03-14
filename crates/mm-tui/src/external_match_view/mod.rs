//! External Matches lateral view — browse AcoustID matches by confidence tier.
//!
//! Flat navigable list with section headers:
//! - **Actions**: Cache external metadata matches, Analyze release matches
//! - **Matches**: Untagged + confidence-bucketed entries
//! - **Release Packing**: Category entries from bin-packing analysis
//!
//! Enter on a match bucket launches the existing `external_match_modal` review flow.
//! Z opens wizard popup with detail info for the selected entry.

pub mod render;

use std::collections::BTreeSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use mm_meta::views::{ConfidenceTier, ExternalMatchesData};
use crate::release_packing_browser::types::PackingCategory;
use crate::widgets::standard_list::ListEntry;
use crate::widgets::wizard::{WizardItem, WizardOffer};

// ============================================================================
// Actions
// ============================================================================

/// Domain actions produced by key dispatch.
///
/// Protocol actions (CycleNext, CyclePrev, Cancel-as-quit) are handled centrally.
pub(crate) enum ExternalMatchesAction {
    /// Enter on "Cache external metadata matches" entry
    RequestFetch,
    /// Enter on "Analyze release matches" entry
    RequestReleasePacking,
    /// Enter on "Untagged matches" → launch review for untagged entries
    LaunchUntaggedReview,
    /// Enter on a confidence bucket → launch review for entries in that tier
    LaunchTierReview(ConfidenceTier),
    /// Enter on a release packing category → launch browser for that category
    LaunchPackingCategory(PackingCategory),
}

// ============================================================================
// Navigable Entry
// ============================================================================

/// A navigable entry type (used as confirm action and rendering discriminant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NavigableEntry {
    /// "Cache external metadata matches" action entry (always present)
    FetchAction,
    /// "Analyze release matches" action entry (always present)
    PackReleasesAction,
    /// "Untagged matches" — files with fingerprint hits but no existing tags
    UntaggedMatches,
    /// Confidence tier bucket
    ConfidenceBucket(ConfidenceTier),
    /// Release packing category entry
    PackingCategory(PackingCategory),
}

// ============================================================================
// List Item (flat items for StandardList)
// ============================================================================

/// A single item in the flat StandardList (headers + entries + info lines).
pub(crate) enum ExternalMatchListItem {
    /// Non-selectable section header
    Header(String),
    /// Empty spacer line
    Spacer,
    /// Navigable entry with wizard detail
    Entry {
        nav: NavigableEntry,
        detail_lines: Vec<Line<'static>>,
    },
    /// Non-selectable info line (e.g., VA override count)
    InfoLine(Line<'static>),
}

impl WizardItem for ExternalMatchListItem {
    fn wizard(&self, _width: u16) -> Option<WizardOffer> {
        match self {
            Self::Entry { detail_lines, .. } if !detail_lines.is_empty() => {
                Some(WizardOffer::Popup(detail_lines.clone()))
            }
            _ => None,
        }
    }
}

impl ListEntry for ExternalMatchListItem {
    type Action = NavigableEntry;

    fn on_confirm(&self, _selected: &BTreeSet<usize>) -> Option<NavigableEntry> {
        match self {
            Self::Entry { nav, .. } => Some(nav.clone()),
            _ => None,
        }
    }

    fn is_selectable(&self) -> bool {
        matches!(self, Self::Entry { .. })
    }
}

// ============================================================================
// State
// ============================================================================

/// Re-export interaction type from mm-ui.
pub use mm_ui::view_state::lateral::external_matches::ExternalMatchesInteraction;

/// Server-fetched data for the External Matches lateral view.
///
/// Interaction state (list cursor, tick counter) lives separately in
/// [`ExternalMatchesInteraction`].
pub(crate) struct ExternalMatchesViewData {
    /// Cached data from the cache thread
    pub cached_data: Option<ExternalMatchesData>,
    /// Pre-built flat items for StandardList
    pub flat_items: Vec<ExternalMatchListItem>,
    /// Whether the Witch has an active fetch batch
    pub fetch_active: bool,
    /// Whether an AcoustID API key is configured
    pub has_api_key: bool,
    /// Latest fetch progress snapshot from the Witch
    pub fetch_progress: Option<mm_meta::witch_types::FetchProgress>,
    /// Whether to show singles before incompletes in the menu (from config).
    pub singles_before_incompletes: bool,
}

// ============================================================================
// Construction & Update
// ============================================================================

impl ExternalMatchesViewData {
    pub fn new(fetch_active: bool, has_api_key: bool, singles_before_incompletes: bool) -> Self {
        let mut data = Self {
            cached_data: None,
            flat_items: Vec::new(),
            fetch_active,
            has_api_key,
            fetch_progress: None,
            singles_before_incompletes,
        };
        data.rebuild_items();
        data
    }

    /// Update cached data from cache thread.
    /// Returns true (caller should clamp interaction cursor).
    pub fn update(&mut self, data: ExternalMatchesData) -> bool {
        self.cached_data = Some(data);
        self.rebuild_items();
        true
    }

    /// Rebuild flat items from current state. Call after any state change
    /// that affects the item list (cached_data, fetch_active, has_api_key).
    pub fn rebuild_items(&mut self) {
        let mut items: Vec<ExternalMatchListItem> = Vec::new();

        // ── Actions section ──
        items.push(ExternalMatchListItem::Header("Actions".to_string()));

        items.push(ExternalMatchListItem::Entry {
            nav: NavigableEntry::FetchAction,
            detail_lines: self.fetch_detail_lines(),
        });

        items.push(ExternalMatchListItem::Entry {
            nav: NavigableEntry::PackReleasesAction,
            detail_lines: self.pack_releases_detail_lines(),
        });

        if let Some(ref data) = self.cached_data {
            let has_matches =
                !data.untagged_entries.is_empty() || !data.confidence_buckets.is_empty();
            if has_matches {
                items.push(ExternalMatchListItem::Spacer);
                items.push(ExternalMatchListItem::Header("Matches".to_string()));

                // Untagged matches
                if !data.untagged_entries.is_empty() {
                    items.push(ExternalMatchListItem::Entry {
                        nav: NavigableEntry::UntaggedMatches,
                        detail_lines: self.untagged_detail_lines(),
                    });
                }

                // Confidence tiers
                for bucket in &data.confidence_buckets {
                    items.push(ExternalMatchListItem::Entry {
                        nav: NavigableEntry::ConfidenceBucket(bucket.tier),
                        detail_lines: self.tier_detail_lines(bucket.tier),
                    });
                }

                // Release packing categories
                let has_packing = data.packing_perfect_count > 0
                    || data.packing_full_match_count > 0
                    || data.packing_singles_count > 0
                    || data.packing_incomplete_count > 0
                    || data.packing_low_confidence_count > 0
                    || data.packing_knots_count > 0
                    || data.unsolved_conflict_count > 0
                    || data.unsolved_no_release_count > 0
                    || data.unsolved_no_match_count > 0;

                if has_packing {
                    items.push(ExternalMatchListItem::Spacer);
                    items.push(ExternalMatchListItem::Header(
                        "Release Packing".to_string(),
                    ));

                    let incomplete = (PackingCategory::Incomplete, data.packing_incomplete_count);
                    let singles = (PackingCategory::Singles, data.packing_singles_count);

                    let mut packing_order: Vec<(PackingCategory, usize)> = vec![
                        (PackingCategory::Perfect, data.packing_perfect_count),
                        (PackingCategory::FullMatches, data.packing_full_match_count),
                    ];
                    if self.singles_before_incompletes {
                        packing_order.push(singles);
                        packing_order.push(incomplete);
                    } else {
                        packing_order.push(incomplete);
                        packing_order.push(singles);
                    }
                    packing_order.extend([
                        (
                            PackingCategory::LowConfidence,
                            data.packing_low_confidence_count,
                        ),
                        (PackingCategory::Knots, data.packing_knots_count),
                        (
                            PackingCategory::UnsolvedConflict,
                            data.unsolved_conflict_count,
                        ),
                        (
                            PackingCategory::UnsolvedNoRelease,
                            data.unsolved_no_release_count,
                        ),
                        (
                            PackingCategory::UnsolvedNoMatch,
                            data.unsolved_no_match_count,
                        ),
                    ]);

                    for (cat, count) in packing_order {
                        if count > 0 {
                            items.push(ExternalMatchListItem::Entry {
                                nav: NavigableEntry::PackingCategory(cat),
                                detail_lines: packing_category_detail_lines(cat),
                            });
                        }
                    }

                    // Pinned release conflict info (non-navigable, critical)
                    if data.pinned_conflict_count > 0 {
                        items.push(ExternalMatchListItem::InfoLine(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled("✗ ", Style::default().fg(Color::Red)),
                            Span::styled(
                                format!(
                                    "{} pinned release conflict{}",
                                    data.pinned_conflict_count,
                                    if data.pinned_conflict_count == 1 { "" } else { "s" }
                                ),
                                Style::default().fg(Color::Red),
                            ),
                        ])));
                    }

                    // VA override info (non-navigable)
                    if data.va_override_count > 0 {
                        items.push(ExternalMatchListItem::InfoLine(Line::from(vec![
                            Span::styled("  ", Style::default()),
                            Span::styled("⚠ ", Style::default().fg(Color::Yellow)),
                            Span::styled(
                                format!("{} VA overrides", data.va_override_count),
                                Style::default().fg(Color::Yellow),
                            ),
                        ])));
                    }
                }
            }
        }

        self.flat_items = items;
    }

    // ── Detail line generators (baked into items at construction) ──

    fn fetch_detail_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(Span::styled(
                "Cache external metadata matches",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];

        if !self.has_api_key {
            lines.push(Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled("No API Key", Style::default().fg(Color::Red)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Configure an AcoustID API key",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "in Config to enable lookups.",
                Style::default().fg(Color::DarkGray),
            )));
        } else if self.fetch_active {
            if let Some(ref p) = self.fetch_progress {
                let a = &p.acoustid;
                let m = &p.mb;

                if a.total > 0 {
                    lines.push(Line::from(vec![
                        Span::styled("AcoustID:  ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{}/{}", a.processed, a.total),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("  Matched:    ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{:>5}", a.matched),
                            Style::default().fg(Color::Green),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("  No match:   ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{:>5}", a.no_match),
                            Style::default().fg(Color::White),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("  Retries:    ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{:>5}", a.retries),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]));
                }

                if m.total > 0 {
                    if a.total > 0 {
                        lines.push(Line::from(""));
                    }
                    lines.push(Line::from(vec![
                        Span::styled("MusicBrainz: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{}/{}", m.processed, m.total),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("  Good fetch: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{:>5}", m.matched),
                            Style::default().fg(Color::Green),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("  Not found:  ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{:>5}", m.no_match),
                            Style::default().fg(Color::White),
                        ),
                    ]));
                    lines.push(Line::from(vec![
                        Span::styled("  Retries:    ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{:>5}", m.retries),
                            Style::default().fg(Color::Yellow),
                        ),
                    ]));
                }

                // Text progress (no animated bar in wizard popup)
                let total_processed = a.processed + m.processed;
                let total_items = a.total + m.total;
                if total_items > 0 {
                    let pct = ((total_processed as f32 / total_items as f32) * 100.0).round();
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("  Progress: {}%", pct),
                        Style::default().fg(Color::Yellow),
                    )));
                }

                // ETA
                let a_remaining = a.total.saturating_sub(a.processed);
                let m_remaining = m.total.saturating_sub(m.processed);
                let a_rps = p.acoustid_rps.max(0.1);
                let m_rps = p.mb_rps.max(0.1);
                let secs = (a_remaining as f32 / a_rps + m_remaining as f32 / m_rps) as u64;
                if secs > 0 {
                    let eta = if secs >= 3600 {
                        format!("{}h {:02}m", secs / 3600, (secs % 3600) / 60)
                    } else if secs >= 60 {
                        format!("{}m {:02}s", secs / 60, secs % 60)
                    } else {
                        format!("{}s", secs)
                    };
                    lines.push(Line::from(Span::styled(
                        format!("  ETA: ~{}", eta),
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            } else {
                lines.push(Line::from(vec![
                    Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                    Span::styled("Active", Style::default().fg(Color::Yellow)),
                ]));
            }
        } else {
            lines.push(Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled("Idle", Style::default().fg(Color::Green)),
            ]));
            lines.push(Line::from(vec![
                Span::styled("API Key: ", Style::default().fg(Color::DarkGray)),
                Span::styled("configured", Style::default().fg(Color::Green)),
            ]));

            if let Some(ref p) = self.fetch_progress {
                let a = &p.acoustid;
                let m = &p.mb;
                let total = a.total + m.total;
                if total > 0 {
                    lines.push(Line::from(""));
                    if a.total > 0 {
                        lines.push(Line::from(Span::styled(
                            format!("Last AcoustID: {} processed", a.total),
                            Style::default().fg(Color::DarkGray),
                        )));
                        lines.push(Line::from(vec![
                            Span::styled("  Matched: ", Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                format!("{}", a.matched),
                                Style::default().fg(Color::Green),
                            ),
                            Span::styled("  No match: ", Style::default().fg(Color::DarkGray)),
                            Span::styled(
                                format!("{}", a.no_match),
                                Style::default().fg(Color::White),
                            ),
                        ]));
                    }
                    if m.total > 0 {
                        lines.push(Line::from(Span::styled(
                            format!("Last MB: {} good fetch", m.matched),
                            Style::default().fg(Color::DarkGray),
                        )));
                    }
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to start lookup.",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn pack_releases_detail_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(Span::styled(
                "Analyze release matches",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];

        let has_data = self
            .cached_data
            .as_ref()
            .is_some_and(|d| !d.confidence_buckets.is_empty());

        let stale = self
            .cached_data
            .as_ref()
            .is_some_and(|d| d.pinned_releases_stale);

        if self.fetch_active {
            lines.push(Line::from(Span::styled(
                "Wait for the external fetch to",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "complete before running analysis.",
                Style::default().fg(Color::DarkGray),
            )));
        } else if !has_data {
            lines.push(Line::from(Span::styled(
                "No external match data available.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "Run a fetch first to populate",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "recording and release data.",
                Style::default().fg(Color::DarkGray),
            )));
        } else {
            if stale {
                lines.push(Line::from(Span::styled(
                    "Pinned releases changed — re-run",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(Span::styled(
                    "analysis to update packing results.",
                    Style::default().fg(Color::Yellow),
                )));
                lines.push(Line::from(""));
            }
            lines.push(Line::from(Span::styled(
                "Bin-pack recordings into releases",
                Style::default().fg(Color::White),
            )));
            lines.push(Line::from(Span::styled(
                "using cached MusicBrainz data.",
                Style::default().fg(Color::White),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Scores each file by AcoustID",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "confidence, duration match, tag",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "similarity, and directory cohesion.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to start analysis.",
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn untagged_detail_lines(&self) -> Vec<Line<'static>> {
        let count = self
            .cached_data
            .as_ref()
            .map(|d| d.untagged_entries.len())
            .unwrap_or(0);

        vec![
            Line::from(Span::styled(
                "Untagged Matches",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                format!("{} files with fingerprint matches", count),
                Style::default().fg(Color::White),
            )),
            Line::from(Span::styled(
                "but no existing tags for matched fields.",
                Style::default().fg(Color::White),
            )),
            Line::from(""),
            Line::from(Span::styled(
                "Press Enter to browse.",
                Style::default().fg(Color::DarkGray),
            )),
        ]
    }

    fn tier_detail_lines(&self, tier: ConfidenceTier) -> Vec<Line<'static>> {
        let total = self
            .cached_data
            .as_ref()
            .and_then(|d| d.confidence_buckets.iter().find(|b| b.tier == tier))
            .map(|b| b.total)
            .unwrap_or(0);

        let title = format!("{} Confidence Matches", tier.label());

        let description = match tier {
            ConfidenceTier::Perfect => {
                format!("{} files with fingerprint confidence = 100%.", total)
            }
            ConfidenceTier::VeryHigh => {
                format!("{} files with fingerprint confidence 99%+.", total)
            }
            ConfidenceTier::High => format!("{} files with fingerprint confidence 95%+.", total),
            ConfidenceTier::Medium => format!("{} files with fingerprint confidence 90%+.", total),
            ConfidenceTier::Low => format!("{} files with fingerprint confidence < 90%.", total),
        };

        let hint = match tier {
            ConfidenceTier::Perfect | ConfidenceTier::VeryHigh => {
                "Highest impact matches. Press Enter to browse."
            }
            ConfidenceTier::High => "High confidence matches. Press Enter to browse.",
            ConfidenceTier::Medium => "Medium confidence. Press Enter to browse.",
            ConfidenceTier::Low => "Low confidence. May contain false positives.",
        };

        vec![
            Line::from(Span::styled(
                title,
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(description, Style::default().fg(Color::White))),
            Line::from(""),
            Line::from(Span::styled(hint, Style::default().fg(Color::DarkGray))),
        ]
    }
}

// ============================================================================
// Key Handling
// ============================================================================

impl ExternalMatchesViewData {
    /// Map a list confirm result to a domain action.
    /// Called by the input dispatch after `interaction.list.handle_input()`.
    pub fn map_confirm(&self, nav: NavigableEntry) -> Option<ExternalMatchesAction> {
        match nav {
            NavigableEntry::FetchAction => {
                if self.has_api_key && !self.fetch_active {
                    Some(ExternalMatchesAction::RequestFetch)
                } else {
                    None
                }
            }
            NavigableEntry::PackReleasesAction => {
                let has_data = self.cached_data.as_ref().is_some_and(|d| {
                    !d.untagged_entries.is_empty() || !d.confidence_buckets.is_empty()
                });
                if !self.fetch_active && has_data {
                    Some(ExternalMatchesAction::RequestReleasePacking)
                } else {
                    None
                }
            }
            NavigableEntry::UntaggedMatches => Some(ExternalMatchesAction::LaunchUntaggedReview),
            NavigableEntry::ConfidenceBucket(tier) => {
                Some(ExternalMatchesAction::LaunchTierReview(tier))
            }
            NavigableEntry::PackingCategory(cat) => {
                Some(ExternalMatchesAction::LaunchPackingCategory(cat))
            }
        }
    }
}

// ============================================================================
// Detail Lines (free functions)
// ============================================================================

fn packing_category_detail_lines(cat: PackingCategory) -> Vec<Line<'static>> {
    let (title, description) = match cat {
        PackingCategory::Perfect => (
            "Perfect Matches",
            "Releases where every track was matched via AcoustID fingerprint.",
        ),
        PackingCategory::FullMatches => (
            "Full Matches",
            "Releases where every track has been matched, some via elimination scoring.",
        ),
        PackingCategory::Singles => (
            "Singles",
            "Single-track releases (one matched track, no other slots).",
        ),
        PackingCategory::Incomplete => (
            "Incomplete Releases",
            "Releases with some but not all tracks matched to corpus files.",
        ),
        PackingCategory::UnsolvedConflict => (
            "Unsolved — Lost Conflict Resolution",
            "Files with AcoustID matches that were scored for releases but lost MIS conflict \
             resolution to other files. These had viable release candidates.",
        ),
        PackingCategory::UnsolvedNoRelease => (
            "Unsolved — No Viable Release",
            "Files with AcoustID recording matches whose releases never produced optimal \
             assignments for their directory. Likely fingerprint collisions or releases \
             with too few local candidates.",
        ),
        PackingCategory::UnsolvedNoMatch => (
            "Unsolved — No AcoustID Match",
            "Fingerprinted corpus files with no AcoustID recording match at all. These files \
             have never been submitted to AcoustID, or the service has no match for them.",
        ),
        PackingCategory::LowConfidence => (
            "Low Confidence",
            "Releases where AcoustID coverage is very low and album name match is poor. \
             These are likely mispacks where elimination filled slots on the wrong release.",
        ),
        PackingCategory::Knots => (
            "Packing Knots",
            "Dense conflict components where many releases compete for a small set of files. \
             Resolved greedily by best score — review to verify the algorithm's choices.",
        ),
    };

    vec![
        Line::from(Span::styled(
            title.to_string(),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(
            description.to_string(),
            Style::default().fg(Color::DarkGray),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter to browse.",
            Style::default().fg(Color::DarkGray),
        )),
    ]
}
