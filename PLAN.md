# Implementation Plan: Insights System

## Overview

Build the Insights system as a standalone, full-screen view that computes and presents aggregated signal data to the user. This is the "dashboard" layer over the existing signals infrastructure.

**Scope**: Insights computation and display only. All flow integrations are stubbed.

---

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│  INSIGHTS VIEW (UiMode::Insights)                           │
│  Full-screen, part of lateral view ring                     │
│  Uses existing widgets (SelectableList, HealthStatus, etc.) │
└─────────────────────────────────────────────────────────────┘
            │ renders
            ▼
┌─────────────────────────────────────────────────────────────┐
│  INSIGHTS (computed in corpus/health/insights/)             │
│  • One-Dim: immediate from signals + HeartbeatResult        │
│  • Multi-Dim: background ops with progress                  │
└─────────────────────────────────────────────────────────────┘
            │ reads from
            ▼
┌─────────────────────────────────────────────────────────────┐
│  SIGNALS (existing health_issues table + HeartbeatResult)   │
└─────────────────────────────────────────────────────────────┘
```

---

## Navigation Model: Lateral View Ring

Full-screen views are **peers** that can be cycled between horizontally:

```
         Shift-Tab                              Tab
    ◄─────────────── [Insights] ───────────────►
                          │
    ┌─────────────────────┴─────────────────────┐
    │           Lateral View Ring               │
    │                                           │
    │  [Insights] ◄──► [Deploy Overview] ◄──►   │
    │       ◄──► [Corpus Browser] ◄──► ...      │
    │                                           │
    └───────────────────────────────────────────┘
                          │
                         Esc
                          ▼
                    [Main Menu]
                    (hub/launcher)
```

**Navigation controls**:
- `Tab` / `Shift-Tab`: Cycle between peer views in the ring
- `Esc`: Return to main menu (the hub)
- Arrow keys / Enter: Navigate within current view

**Key principle**: Views are siblings, not parent-child. You cycle laterally between them, and the main menu is the central hub you can always escape back to.

---

## Module Structure

```
src/corpus/health/insights/
├── mod.rs           # Insight enum, InsightComputer, FlowType stub
├── one_dim.rs       # Immediate computations (signals + heartbeat)
└── multi_dim.rs     # Background computations via ops/operation.rs

src/ui/
├── insights_view/
│   ├── mod.rs       # InsightsViewState, handle_key, render
│   └── render.rs    # Rendering using widgets
```

---

## Phase 1: Core Insight Types

### 1.1 Create `corpus/health/insights/mod.rs`

```rust
//! Computed Insights over Health Signals
//!
//! Insights are real-time computed views that aggregate signals into
//! actionable recommendations. They are never stored - always fresh.

mod one_dim;
mod multi_dim;

pub use one_dim::compute_one_dim_insights;
pub use multi_dim::{spawn_multi_dim_insight, MultiDimInsightType};

/// Stub - flows don't exist yet
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowType {
    Placeholder,
}

/// Computed insight - never stored, always fresh
#[derive(Debug, Clone)]
pub enum Insight {
    // === Multi-Dimensional (background, lift to top) ===
    QualityDuplicates {
        dupe_groups: usize,
        total_tracks: usize,
        auto_resolvable: usize,
    },

    // === One-Dimensional (immediate) ===
    DeploymentConflicts { count: usize },
    TagCanonicalization {
        field: String,
        variant_count: usize,
        affected_tracks: usize
    },
    OutOfBandChanges {
        tag_changes: usize,
        file_changes: usize
    },
    IndexDesync {
        missing_from_disk: usize,
        missing_from_index: usize,
        relocated: usize
    },
    MissingTags {
        tag_name: String,
        count: usize
    },

    // === Reassuring Health (bottom) ===
    CorpusHealth {
        total_tracks: usize,
        indexed_healthy: bool,
        libraries_healthy: bool,
        tags_synced: bool,
    },

    // === Computing placeholder ===
    Computing {
        insight_type: MultiDimInsightType,
        progress: Option<f32>,
    },
}

impl Insight {
    /// Priority for display ordering (higher = more prominent)
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

            // Computing shows in-place based on target type
            Insight::Computing { insight_type, .. } => {
                match insight_type {
                    MultiDimInsightType::QualityDuplicates => 100,
                }
            }
        }
    }

    /// Health status for coloring
    pub fn health_status(&self) -> HealthStatus {
        match self {
            Insight::CorpusHealth { indexed_healthy, libraries_healthy, tags_synced, .. } => {
                if *indexed_healthy && *libraries_healthy && *tags_synced {
                    HealthStatus::Healthy
                } else {
                    HealthStatus::Warning
                }
            }
            Insight::IndexDesync { missing_from_disk, .. } if *missing_from_disk > 0 => {
                HealthStatus::Critical
            }
            Insight::DeploymentConflicts { count } if *count > 0 => HealthStatus::Warning,
            Insight::QualityDuplicates { dupe_groups, .. } if *dupe_groups > 0 => HealthStatus::Info,
            Insight::Computing { .. } => HealthStatus::Info,
            _ => HealthStatus::Healthy,
        }
    }

    /// Is this actionable (would launch a flow if flows existed)?
    pub fn is_actionable(&self) -> bool {
        match self {
            Insight::CorpusHealth { .. } => false,
            Insight::Computing { .. } => false,
            _ => self.item_count() > 0,
        }
    }

    /// Number of items this insight represents
    pub fn item_count(&self) -> usize {
        match self {
            Insight::QualityDuplicates { dupe_groups, .. } => *dupe_groups,
            Insight::DeploymentConflicts { count } => *count,
            Insight::TagCanonicalization { variant_count, .. } => *variant_count,
            Insight::OutOfBandChanges { tag_changes, file_changes } => tag_changes + file_changes,
            Insight::IndexDesync { missing_from_disk, missing_from_index, relocated } => {
                missing_from_disk + missing_from_index + relocated
            }
            Insight::MissingTags { count, .. } => *count,
            Insight::CorpusHealth { .. } => 0,
            Insight::Computing { .. } => 0,
        }
    }

    /// Stub: which flow would resolve this
    pub fn flow_type(&self) -> FlowType {
        FlowType::Placeholder
    }

    /// Display label for the insight
    pub fn label(&self) -> String {
        match self {
            Insight::QualityDuplicates { dupe_groups, auto_resolvable, .. } => {
                format!("Quality Duplicates: {} groups ({} auto-resolvable)",
                    dupe_groups, auto_resolvable)
            }
            Insight::DeploymentConflicts { count } => {
                format!("Deployment Conflicts: {}", count)
            }
            Insight::TagCanonicalization { field, variant_count, .. } => {
                format!("{} variants: {} spellings", field, variant_count)
            }
            Insight::OutOfBandChanges { tag_changes, file_changes } => {
                format!("External changes: {} tags, {} files", tag_changes, file_changes)
            }
            Insight::IndexDesync { missing_from_disk, missing_from_index, relocated } => {
                format!("Index sync: {} missing, {} new, {} moved",
                    missing_from_disk, missing_from_index, relocated)
            }
            Insight::MissingTags { tag_name, count } => {
                format!("Missing {}: {} tracks", tag_name, count)
            }
            Insight::CorpusHealth { total_tracks, .. } => {
                format!("Corpus: {} tracks indexed", total_tracks)
            }
            Insight::Computing { insight_type, progress } => {
                let pct = progress.map(|p| format!(" {:.0}%", p * 100.0)).unwrap_or_default();
                format!("Computing {:?}...{}", insight_type, pct)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MultiDimInsightType {
    QualityDuplicates,
    // Future: TagConflictDuplicates, etc.
}
```

### 1.2 Create `corpus/health/insights/one_dim.rs`

One-dimensional insights computed immediately from signals + heartbeat:

- `CorpusHealth` from HeartbeatResult
- `IndexDesync` from HeartbeatResult (process relocations first!)
- `DeploymentConflicts` from health_issues query
- `TagCanonicalization` per field from tag_canonicalization table
- `OutOfBandChanges` from health_issues query
- `MissingTags` from health_issues query

**Key**: HeartbeatResult is cloned in, relocations processed first to filter superseded signals.

### 1.3 Create `corpus/health/insights/multi_dim.rs`

Background computation using existing `ops/operation.rs` infrastructure:

- `QualityDuplicates`: correlate FingerprintDuplicate + QualityVariant signals in Rust

Returns `OperationHandle` for progress tracking.

---

## Phase 2: Eye Animation Enhancements

### 2.1 Add heartbeat scheduling to `EyeAnimation`

```rust
impl EyeAnimation {
    /// Guarantee heartbeat on next blink (lazy scheduling)
    pub fn schedule_heartbeat(&mut self) {
        self.force_next_heartbeat = true;
    }

    /// Trigger immediate flutter blink (for insights initialization)
    pub fn trigger_flutter(&mut self) {
        self.current_blink_type = BlinkType::Flutter;
        self.flutter_count = 3;
        self.state = EyeAnimationState::Closing;
        self.state_start_time = Instant::now();
    }
}
```

### 2.2 Add `force_next_heartbeat` field

When set, next blink completion always triggers heartbeat (bypasses d20 roll).

---

## Phase 3: UI View

### 3.1 Add `UiMode::Insights`

Full-screen insights view, part of the lateral view ring.

### 3.2 Create `ui/insights_view/mod.rs`

```rust
pub struct InsightsViewState {
    /// Computed insights (sorted by priority, multi-dim at top)
    pub insights: Vec<Insight>,
    /// List selection state
    pub list_state: SelectableListState,
    /// Background computation handles
    pub pending_computations: Vec<(MultiDimInsightType, OperationHandle)>,
    /// Cached heartbeat result
    pub heartbeat: Option<HeartbeatResult>,
}
```

**Key handlers**:
- `enter()`: Trigger flutter, compute one-dim, spawn multi-dim
- `handle_key()`:
  - `Tab` / `Shift-Tab`: Cycle to adjacent view (stub: no-op for now)
  - `Esc`: Return to main menu
  - `Up` / `Down`: Navigate list
  - `Enter` on actionable: No-op (flows don't exist yet)
- `update()`: Poll pending computations, merge completed insights, re-sort

### 3.3 Create `ui/insights_view/render.rs`

Use existing widgets:
- `SelectableList` for insight items
- `HealthStatus` colors via `SelectableItem.with_style()`
- `StatusIndicator` for computing progress
- Layout: Eye panel persistent (left or top), insights list as main content

---

## Phase 4: Integration

### 4.1 Wire into main UI dispatch

- Add `UiMode::Insights` handling in `ui/mod.rs`
- Add menu command to launch insights view
- Render eye panel in insights view (make it persistent across views)

### 4.2 Update `corpus/health/mod.rs`

Export the new insights module.

### 4.3 Signal refresh after mutations

When mutations complete, call `eye_animation.schedule_heartbeat()` to refresh signals → insights will reflect changes on next view entry.

---

## Documentation Updates

### CLAUDE.md additions:

```markdown
### Widget-First UI Development

When building new UI components:
1. **Always check existing widgets first** in `ui/widgets/`
2. If no suitable widget exists, **create a new widget first**
3. Then use the widget in your view

Never inline widget-like rendering code directly in views.

### Vestigial Code

The following modules are vestigial and should not be referenced:
- `corpus/db/decisions.rs` - will be rebuilt in flows domain
- `ui/*_flow/` directories - flows will be rebuilt from scratch
- `ui/flows/` - placeholder only
```

---

## Implementation Order

1. **Phase 1.1**: Create insight types in `corpus/health/insights/mod.rs`
2. **Phase 1.2**: Implement one-dim computations
3. **Phase 1.3**: Implement multi-dim background computation
4. **Phase 2**: Eye animation enhancements
5. **Phase 3.1-3.2**: InsightsViewState and handlers
6. **Phase 3.3**: Rendering with widgets
7. **Phase 4**: Integration and wiring

---

## Testing Strategy

- Unit tests for insight computation (mock signal data)
- Integration test: signals in DB → compute_one_dim → expected insights
- Manual testing: Full UI flow with eye animation

---

## Out of Scope (Stubbed)

- `FlowType` enum - placeholder only
- `Insight.flow_type()` - returns `FlowType::Placeholder`
- Launching flows from insights - no-op
- `DecisionSource` trait - not implemented yet
- `notify_pending_mutation` callback - future work
- Lateral view cycling - handlers exist but no-op until other views exist
