//! Reusable UI Widget Primitives
//!
//! This module provides foundational UI components used across MLA's interface.
//! These widgets encapsulate common patterns for layout, modals, controls hints,
//! and text input.
//!
//! ## Design Principles
//!
//! - **Composable**: Widgets can be nested and combined
//! - **Configurable**: Builder patterns for flexible customization
//! - **Consistent**: Uniform styling and behavior across the app
//! - **Focus-aware**: Widgets know when they're focused and style accordingly

mod controls;
mod layout;
mod list_click_targets;
mod modal;
mod resolution_layout;
pub mod signal_info_pane;
pub mod tabbed_signal_list;
mod text_input;
mod titlebar;

// Layout widgets
pub use layout::{PaneConfig, ThreePaneLayout};
pub use list_click_targets::ListClickTargets;
pub use resolution_layout::{ButtonRects, FocusPane, ResolutionLayout};

// Modal widgets
pub use modal::{centered_rect, centered_rect_fixed, Modal, ModalButton, ModalStyle};

// Controls hint widgets
pub use controls::presets as control_presets;

// Title bar widgets
pub use titlebar::{LateralView, UnifiedTitleBar};

// Text input widgets
pub use text_input::TextInputState;

// Deploy signal widgets
pub use signal_info_pane::{SignalInfo, SignalInfoPane};
pub use tabbed_signal_list::{DeployTab, TabbedSignalList};
