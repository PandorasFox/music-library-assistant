//! Progressive Worker Modal - Timed Bulk Operations
//!
//! This module provides a UI pattern for long operations that must run on the UI thread
//! (e.g., staging mutations via DecisionWitness). Work is processed in timed chunks
//! (~50ms per tick) with progress feedback.
//!
//! ## Design
//!
//! - Takes over the event loop with `UiMode::ProgressiveWork`
//! - Processes items in timed chunks (20fps is fine for TUI)
//! - Shows progress bar with item count and current item label
//! - Handles NO input - purely displays progress
//! - Drains input buffer before yielding control back
//!
//! ## Integration
//!
//! To use progressive worker:
//! 1. Create `ProgressiveWorkerState` with work items and completion handler
//! 2. Set `app.progressive_worker = Some(state)`
//! 3. Set `app.mode = UiMode::ProgressiveWork`
//! 4. The tick function processes items until done
//! 5. On completion, the handler transitions to the appropriate next state

mod types;
mod render;

pub use types::{
    OnComplete,
    ProgressiveWorkerState,
    WorkItem,
    WorkSummary,
};
pub use render::render;
