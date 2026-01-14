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
mod titlebar;

// Layout widgets
pub use layout::{FocusablePane, PaneConfig, PaneStyle, ThreePaneLayout, TwoPaneLayout};

// Modal widgets
pub use modal::{centered_rect, ConfirmationModal, Modal, ModalButton, ModalStyle, ScrollableModal};

// Controls hint widgets
pub use controls::{presets as control_presets, ControlsHint, ControlsStyle, KeyBinding};

// List widgets
pub use list::{SelectableItem, SelectableList, SelectableListState, SelectableListStyle, SimpleList};

// Status widgets
pub use status::{HealthStatus, StatusColor, StatusIndicator};

// Title bar widgets
pub use titlebar::{LateralView, UnifiedTitleBar};
