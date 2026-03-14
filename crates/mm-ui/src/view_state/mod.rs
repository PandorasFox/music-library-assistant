//! View state types for route-based UI architecture.
//!
//! Each view in MM has three layers:
//!
//! 1. **Route** (in [`crate::route`]) — URL-encodable navigation + position state.
//! 2. **Interaction** (this module) — backend-agnostic state needed for input
//!    handling and rendering, composed from primitives like [`StandardListState`],
//!    [`TextInputState`], [`ButtonRowState`].
//! 3. **Data** (in `mm-meta`) — server-fetched query responses, immutable during
//!    interaction.
//!
//! Both TUI and web clients use the same Route and Interaction types.
//! Data is fetched separately per backend (TUI: protocol query, web: HTTP fetch).
//!
//! ## ViewCore Trait
//!
//! All interaction structs implement [`ViewCore`], providing:
//! - `from_route()` — initialize from URL-encoded state
//! - `to_route()` — extract URL-encodable state for URL display / bookmarking
//! - `handle_input()` — backend-agnostic input handling (may need data context)
//!
//! ## Data Refresh Pattern
//!
//! When server data changes under a live view:
//! ```text
//! let route = interaction.to_route();
//! interaction = Interaction::from_route(&route);
//! interaction.clamp_to_data(new_len);
//! ```
//! The Route acts as a stable bookmark across data refreshes.

pub mod lateral;
pub mod overlay;

use crate::input::InputAction;

/// Backend-agnostic trait for view interaction state.
///
/// Every view's interaction struct implements this, enabling shared
/// navigation logic, route synchronization, and input handling
/// across TUI and web clients.
pub trait ViewCore: Sized {
    /// The route sub-type for this view (e.g., `HealthRoute`, `ConfigRoute`).
    type Route;

    /// Action produced by input handling. Domain-specific per view.
    type Action;

    /// Context from server-fetched data needed for input handling.
    /// Use `()` for views where input handling is data-independent.
    type Data;

    /// Initialize interaction state from URL-encoded route parameters.
    /// Cursor/scroll values from the route are set directly; call
    /// `clamp_to_data` afterward to ensure they're within valid range.
    fn from_route(route: &Self::Route) -> Self;

    /// Extract URL-encodable position state for URL display and bookmarking.
    fn to_route(&self) -> Self::Route;

    /// Handle a semantic input action with data context.
    ///
    /// Returns `Some(action)` for domain actions (launch modal, confirm, etc.),
    /// `None` for consumed navigation or unhandled input.
    fn handle_input(&mut self, action: &InputAction, data: &Self::Data) -> Option<Self::Action>;
}
