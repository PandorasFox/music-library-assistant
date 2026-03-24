//! External Matches view state: data + interaction bundled.
//!
//! Browse AcoustID matches by confidence tier. The interaction is a
//! single StandardListState plus an animation tick counter.
//!
//! Flat navigable list with section headers:
//! - **Actions**: Cache external metadata matches, Analyze release matches, Download cover art
//! - **Matches**: Untagged + confidence-bucketed entries
//! - **Release Packing**: Category entries from bin-packing analysis
//!
//! Enter on a match bucket launches the existing `external_match_modal` review flow.
//! Z opens wizard popup with detail info for the selected entry.

use std::collections::BTreeSet;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use mm_meta::signals::packing_category::PackingCategory;
use mm_meta::views::{ConfidenceTier, ExternalMatchesData};

use crate::input::InputAction;
use crate::route::ExternalMatchesRoute;
use crate::standard_list::{ListEntry, ListInputResult, StandardListConfig, StandardListState};
use crate::view_state::ViewCore;
use crate::wizard::{WizardItem, WizardOffer};

// ============================================================================
// Actions
// ============================================================================

/// Domain actions produced by key dispatch.
///
/// Protocol actions (CycleNext, CyclePrev, Cancel-as-quit) are handled centrally.
pub enum ExternalMatchesAction {
    /// Enter on "Cache external metadata matches" entry
    RequestFetch,
    /// Enter on "Analyze release matches" entry
    RequestReleasePacking,
    /// Enter on "Download cover art" entry
    RequestCoverArt,
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
pub enum NavigableEntry {
    /// "Cache external metadata matches" action entry (always present)
    FetchAction,
    /// "Analyze release matches" action entry (always present)
    PackReleasesAction,
    /// "Download cover art" action entry (always present)
    CoverArtAction,
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
pub enum ExternalMatchListItem {
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
// ExternalMatchesViewData — server-fetched data
// ============================================================================

/// Server-fetched data for the External Matches lateral view.
///
/// Interaction state (list cursor, tick counter) lives separately in
/// [`ExternalMatchesInteraction`].
pub struct ExternalMatchesViewData {
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
    /// Whether a cover art fetch is currently active.
    pub cover_art_active: bool,
    /// Whether the `cover_art_fetch` config option is enabled.
    pub cover_art_enabled: bool,
    /// Latest cover art fetch progress snapshot.
    pub cover_art_progress: Option<mm_meta::witch_types::CoverArtProgress>,
}

// ============================================================================
// Construction & Update
// ============================================================================

impl ExternalMatchesViewData {
    pub fn new(
        fetch_active: bool,
        has_api_key: bool,
        singles_before_incompletes: bool,
        cover_art_active: bool,
        cover_art_enabled: bool,
    ) -> Self {
        let mut data = Self {
            cached_data: None,
            flat_items: Vec::new(),
            fetch_active,
            has_api_key,
            fetch_progress: None,
            singles_before_incompletes,
            cover_art_active,
            cover_art_enabled,
            cover_art_progress: None,
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

        items.push(ExternalMatchListItem::Entry {
            nav: NavigableEntry::CoverArtAction,
            detail_lines: self.cover_art_detail_lines(),
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

    fn cover_art_detail_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![
            Line::from(Span::styled(
                "Download cover art",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];

        if !self.cover_art_enabled {
            lines.push(Line::from(vec![
                Span::styled("Status: ", Style::default().fg(Color::DarkGray)),
                Span::styled("Disabled", Style::default().fg(Color::DarkGray)),
            ]));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enable cover_art_fetch in Config",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "to download album art from the",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "Cover Art Archive.",
                Style::default().fg(Color::DarkGray),
            )));
        } else if self.cover_art_active {
            if let Some(ref p) = self.cover_art_progress {
                lines.push(Line::from(vec![
                    Span::styled("Releases: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{}/{}", p.processed, p.total_releases),
                        Style::default().fg(Color::Yellow),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Written:  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:>5}", p.images_written),
                        Style::default().fg(Color::Green),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Skipped:  ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:>5}", p.images_skipped),
                        Style::default().fg(Color::White),
                    ),
                ]));
                lines.push(Line::from(vec![
                    Span::styled("  Upgraded: ", Style::default().fg(Color::DarkGray)),
                    Span::styled(
                        format!("{:>5}", p.images_upgraded),
                        Style::default().fg(Color::Cyan),
                    ),
                ]));

                if p.total_releases > 0 {
                    let pct = ((p.processed as f32 / p.total_releases as f32) * 100.0).round();
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("  Progress: {}%", pct),
                        Style::default().fg(Color::Yellow),
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

            if let Some(ref p) = self.cover_art_progress {
                if p.total_releases > 0 {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        format!("Last run: {} releases", p.total_releases),
                        Style::default().fg(Color::DarkGray),
                    )));
                    lines.push(Line::from(vec![
                        Span::styled("  Written: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{}", p.images_written),
                            Style::default().fg(Color::Green),
                        ),
                        Span::styled("  Skipped: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{}", p.images_skipped),
                            Style::default().fg(Color::White),
                        ),
                        Span::styled("  Upgraded: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("{}", p.images_upgraded),
                            Style::default().fg(Color::Cyan),
                        ),
                    ]));
                }
            }

            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Fetch album art from the Cover",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(Span::styled(
                "Art Archive for packed releases.",
                Style::default().fg(Color::DarkGray),
            )));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press Enter to start download.",
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

    /// Map a list confirm result to a domain action.
    /// Called by `ExternalMatchesViewState::handle_input()`.
    fn map_confirm(&self, nav: NavigableEntry) -> Option<ExternalMatchesAction> {
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
            NavigableEntry::CoverArtAction => {
                if self.cover_art_enabled && !self.cover_art_active {
                    Some(ExternalMatchesAction::RequestCoverArt)
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

// ============================================================================
// ExternalMatchesInteraction — UI navigation state
// ============================================================================

/// Interaction state for the external matches view.
pub struct ExternalMatchesInteraction {
    pub list: StandardListState,
    /// Animation tick counter (incremented each UI tick while fetch is active).
    pub tick_count: u32,
}

impl ViewCore for ExternalMatchesInteraction {
    type Route = ExternalMatchesRoute;
    type Action = (); // input handling is on the ViewState
    type Data = ();

    fn from_route(route: &ExternalMatchesRoute) -> Self {
        let mut list = StandardListState::new(StandardListConfig::default());
        if let Some(cursor) = route.cursor {
            list.cursor = cursor;
        }
        Self { list, tick_count: 0 }
    }

    fn to_route(&self) -> ExternalMatchesRoute {
        ExternalMatchesRoute {
            cursor: if self.list.cursor > 0 {
                Some(self.list.cursor)
            } else {
                None
            },
        }
    }

    fn handle_input(&mut self, _action: &InputAction, _data: &()) -> Option<()> {
        None
    }
}

impl ExternalMatchesInteraction {
    pub fn new() -> Self {
        Self {
            list: StandardListState::new(StandardListConfig::default()),
            tick_count: 0,
        }
    }

    /// Clamp cursor position to valid range after data refresh.
    pub fn clamp_to_data(&mut self, items: &[ExternalMatchListItem]) {
        self.list.clamp_cursor(items);
    }
}

// ============================================================================
// ExternalMatchesViewState — bundled data + interaction
// ============================================================================

/// Complete view state for the external matches view.
///
/// Bundles server-fetched data with interaction state so that ActiveView
/// carries a single struct instead of loose `{ data, interaction }` fields.
pub struct ExternalMatchesViewState {
    pub data: ExternalMatchesViewData,
    pub interaction: ExternalMatchesInteraction,
}

impl ExternalMatchesViewState {
    /// Create a new view state from fetched data.
    pub fn new(data: ExternalMatchesViewData) -> Self {
        let mut interaction = ExternalMatchesInteraction::new();
        interaction.clamp_to_data(&data.flat_items);
        Self { data, interaction }
    }

    /// Handle a semantic input action, returning a domain action if one was produced.
    ///
    /// Combines list input handling with the map_confirm logic that gates
    /// actions based on fetch/API key state.
    pub fn handle_input(&mut self, action: &InputAction) -> Option<ExternalMatchesAction> {
        let result = self.interaction.list.handle_input(action, &self.data.flat_items);

        match result {
            ListInputResult::Confirm(nav) => self.data.map_confirm(nav),
            ListInputResult::Consumed
            | ListInputResult::CursorMoved
            | ListInputResult::Toggled
            | ListInputResult::Unhandled => None,
        }
    }

    /// Route serialization.
    pub fn to_route(&self) -> ExternalMatchesRoute {
        self.interaction.to_route()
    }

    /// Restore cursor position from a route.
    pub fn apply_route(&mut self, route: &ExternalMatchesRoute) {
        if let Some(cursor) = route.cursor {
            self.interaction.list.cursor = cursor;
        }
        self.interaction.clamp_to_data(&self.data.flat_items);
    }
}
