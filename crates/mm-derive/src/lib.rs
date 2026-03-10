use proc_macro::TokenStream;

mod wizard_item;

/// Derive macro for the `WizardItem` trait.
///
/// Generates `wizard(&self, width: u16) -> Option<WizardOffer>` by matching
/// on enum variants or struct-level `#[wizard(...)]` attributes.
///
/// # Enum Usage
///
/// Every variant MUST have a `#[wizard(...)]` attribute:
///
/// ```ignore
/// #[derive(WizardItem)]
/// enum MyItem {
///     #[wizard(skip)]
///     Header { title: String },
///
///     #[wizard(popup)]
///     Info {
///         #[wizard(popup_content)]
///         summary: Vec<Line<'static>>,
///     },
///
///     #[wizard(pane)]
///     Detail {
///         #[wizard(pane_title)]
///         title: String,
///         #[wizard(pane_content)]
///         lines: Vec<Line<'static>>,
///     },
///
///     #[wizard(both)]
///     Full {
///         #[wizard(popup_content)]
///         summary: Vec<Line<'static>>,
///         #[wizard(pane_title)]
///         title: String,
///         #[wizard(pane_content)]
///         lines: Vec<Line<'static>>,
///     },
/// }
/// ```
///
/// # Struct Usage
///
/// Place the wizard mode on the struct itself:
///
/// ```ignore
/// #[derive(WizardItem)]
/// #[wizard(popup)]
/// struct TrackInfo {
///     name: String,
///     #[wizard(popup_content)]
///     info_lines: Vec<Line<'static>>,
/// }
/// ```
#[proc_macro_derive(WizardItem, attributes(wizard))]
pub fn derive_wizard_item(input: TokenStream) -> TokenStream {
    wizard_item::derive(input)
}
