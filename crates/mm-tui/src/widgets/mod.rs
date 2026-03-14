//! Reusable UI Widget Primitives
//!
//! This module provides foundational UI components used across MM's interface.
//! These widgets encapsulate common patterns for layout, modals, controls hints,
//! and text input.
//!
//! ## Design Principles
//!
//! - **Composable**: Widgets can be nested and combined
//! - **Configurable**: Builder patterns for flexible customization
//! - **Consistent**: Uniform styling and behavior across the app
//! - **Focus-aware**: Widgets know when they're focused and style accordingly

pub mod album_art_preview;
pub mod control_colors;
pub mod detail_panel;
pub mod file_path_list;
mod layout;
mod list_click_targets;
mod modal;
pub mod modal_buttons;
pub mod modal_frame;
pub mod path_display;
mod resolution_layout;
pub mod selection_styles;
pub mod signal_info_pane;
pub mod standard_list;
pub mod status_bar;
pub mod tabbed_signal_list;
mod text_input;
pub mod rich_text;
pub mod three_col_table;
mod titlebar;
pub mod wizard;
pub mod wizard_pane;
pub mod wizard_popup;

// Layout widgets
pub use layout::{PaneConfig, ThreePaneLayout};
pub use list_click_targets::ListClickTargets;
pub use resolution_layout::{rect_contains, ButtonRects, FocusPane, ResolutionLayout};

// Modal widgets
pub use modal::{
    centered_rect_fixed, render_button_row, ConfirmationButton, ConfirmationModal, Modal,
    ModalButton, ModalStyle,
};

// Title bar widgets
pub use titlebar::{LateralView, UnifiedTitleBar};

// Text input widgets
pub use text_input::TextInputState;

// Deploy signal widgets
pub use signal_info_pane::{SidecarSummary, SignalInfo, SignalInfoPane};
pub use tabbed_signal_list::{DeployTab, TabbedSignalList};

// Path display
pub use path_display::PathField;

// Album art preview
pub use album_art_preview::{
    render_album_art_preview, render_no_art_placeholder, AlbumArtCache, AlbumArtPicker, ArtCacheKey,
};

// File path list + selection styles
pub use file_path_list::{render_file_path_list, PathEntry};
pub use selection_styles::{CURSOR_STYLE, LIST_ITEM_STYLE};

// Three-column table
pub use three_col_table::{StyledCell, ThreeColTable};

// Wizard system
pub use wizard::{WizardItem, WizardOffer, WizardState};

// Modal buttons
pub use modal_buttons::{ButtonRowState, ModalButtons};

// Modal frame
pub use modal_frame::{FrameInputResult, ModalFrame, ModalFrameCore};
