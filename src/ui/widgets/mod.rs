//! Reusable UI Widget Primitives
//!
//! This module provides foundational UI components used across MLA's interface.
//! These widgets encapsulate common patterns for layout, modals, controls hints,
//! and interactive lists.
//!
//! ## Design Principles
//!
//! - **Composable**: Widgets can be nested and combined
//! - **Configurable**: Builder patterns for flexible customization
//! - **Consistent**: Uniform styling and behavior across the app
//! - **Focus-aware**: Widgets know when they're focused and style accordingly

mod controls;
pub mod footer;
mod layout;
mod list;
mod modal;
mod status;
mod text_input;
mod titlebar;

// Layout widgets
pub use layout::{PaneConfig, ThreePaneLayout};

// Modal widgets
pub use modal::{centered_rect, Modal, ModalButton, ModalStyle};

// Controls hint widgets
pub use controls::{presets as control_presets, ControlsHint};

// List widgets
pub use list::{SelectableItem, SelectableList, SelectableListState, SelectableListStyle};

// Status widgets
pub use status::HealthStatus;

// Title bar widgets
pub use titlebar::{LateralView, UnifiedTitleBar};

// Text input widgets
pub use text_input::TextInputState;
