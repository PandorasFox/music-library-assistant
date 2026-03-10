use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::{Data, DeriveInput, Error, Fields, Ident, Result};

pub fn derive(input: TokenStream) -> TokenStream {
    let input = syn::parse_macro_input!(input as DeriveInput);
    match derive_inner(input) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn derive_inner(input: DeriveInput) -> Result<TokenStream2> {
    let name = &input.ident;

    match &input.data {
        Data::Enum(data) => derive_enum(name, data),
        Data::Struct(data) => derive_struct(name, &input, &data.fields),
        Data::Union(_) => Err(Error::new_spanned(
            name,
            "WizardItem cannot be derived for unions",
        )),
    }
}

/// The wizard mode declared on a variant or struct.
#[derive(Clone, Copy, PartialEq, Eq)]
enum WizardMode {
    Skip,
    Popup,
    Pane,
    Both,
}

/// Parse `#[wizard(skip|popup|pane|both)]` from attributes.
fn parse_wizard_mode(attrs: &[syn::Attribute]) -> Result<Option<WizardMode>> {
    for attr in attrs {
        if !attr.path().is_ident("wizard") {
            continue;
        }
        let mode_str = attr.parse_args::<Ident>()?;
        let mode = match mode_str.to_string().as_str() {
            "skip" => WizardMode::Skip,
            "popup" => WizardMode::Popup,
            "pane" => WizardMode::Pane,
            "both" => WizardMode::Both,
            other => {
                return Err(Error::new_spanned(
                    &mode_str,
                    format!(
                        "unknown wizard mode `{other}`, expected one of: skip, popup, pane, both"
                    ),
                ));
            }
        };
        return Ok(Some(mode));
    }
    Ok(None)
}

/// Which wizard field annotation a field has, if any.
#[derive(Clone, Copy, PartialEq, Eq)]
enum FieldRole {
    PopupContent,
    PaneTitle,
    PaneContent,
}

/// Parse `#[wizard(popup_content|pane_title|pane_content)]` from field attributes.
fn parse_field_role(attrs: &[syn::Attribute]) -> Result<Option<FieldRole>> {
    for attr in attrs {
        if !attr.path().is_ident("wizard") {
            continue;
        }
        let role_str = attr.parse_args::<Ident>()?;
        let role = match role_str.to_string().as_str() {
            "popup_content" => FieldRole::PopupContent,
            "pane_title" => FieldRole::PaneTitle,
            "pane_content" => FieldRole::PaneContent,
            other => {
                return Err(Error::new_spanned(
                    &role_str,
                    format!(
                        "unknown wizard field role `{other}`, expected one of: \
                         popup_content, pane_title, pane_content"
                    ),
                ));
            }
        };
        return Ok(Some(role));
    }
    Ok(None)
}

/// Collected field bindings for a variant/struct.
struct FieldBindings {
    popup_content: Option<Ident>,
    pane_title: Option<Ident>,
    pane_content: Option<Ident>,
}

fn collect_field_bindings(fields: &Fields) -> Result<FieldBindings> {
    let mut popup_content = None;
    let mut pane_title = None;
    let mut pane_content = None;

    let named = match fields {
        Fields::Named(f) => f,
        Fields::Unnamed(_) => {
            return Err(Error::new_spanned(
                fields,
                "WizardItem only supports named fields (not tuple variants)",
            ));
        }
        Fields::Unit => {
            return Ok(FieldBindings {
                popup_content: None,
                pane_title: None,
                pane_content: None,
            });
        }
    };

    for field in &named.named {
        if let Some(role) = parse_field_role(&field.attrs)? {
            let ident = field.ident.clone().unwrap();
            match role {
                FieldRole::PopupContent => {
                    if popup_content.is_some() {
                        return Err(Error::new_spanned(
                            field,
                            "duplicate #[wizard(popup_content)]",
                        ));
                    }
                    popup_content = Some(ident);
                }
                FieldRole::PaneTitle => {
                    if pane_title.is_some() {
                        return Err(Error::new_spanned(field, "duplicate #[wizard(pane_title)]"));
                    }
                    pane_title = Some(ident);
                }
                FieldRole::PaneContent => {
                    if pane_content.is_some() {
                        return Err(Error::new_spanned(
                            field,
                            "duplicate #[wizard(pane_content)]",
                        ));
                    }
                    pane_content = Some(ident);
                }
            }
        }
    }

    Ok(FieldBindings {
        popup_content,
        pane_title,
        pane_content,
    })
}

/// Validate that the field bindings match what the mode requires.
fn validate_bindings(
    mode: WizardMode,
    bindings: &FieldBindings,
    span: proc_macro2::Span,
) -> Result<()> {
    match mode {
        WizardMode::Skip => {}
        WizardMode::Popup => {
            if bindings.popup_content.is_none() {
                return Err(Error::new(
                    span,
                    "#[wizard(popup)] requires a #[wizard(popup_content)] field",
                ));
            }
        }
        WizardMode::Pane => {
            if bindings.pane_title.is_none() {
                return Err(Error::new(
                    span,
                    "#[wizard(pane)] requires a #[wizard(pane_title)] field",
                ));
            }
            if bindings.pane_content.is_none() {
                return Err(Error::new(
                    span,
                    "#[wizard(pane)] requires a #[wizard(pane_content)] field",
                ));
            }
        }
        WizardMode::Both => {
            if bindings.popup_content.is_none() {
                return Err(Error::new(
                    span,
                    "#[wizard(both)] requires a #[wizard(popup_content)] field",
                ));
            }
            if bindings.pane_title.is_none() {
                return Err(Error::new(
                    span,
                    "#[wizard(both)] requires a #[wizard(pane_title)] field",
                ));
            }
            if bindings.pane_content.is_none() {
                return Err(Error::new(
                    span,
                    "#[wizard(both)] requires a #[wizard(pane_content)] field",
                ));
            }
        }
    }
    Ok(())
}

/// Generate the match arm body for a given mode + bindings.
fn gen_arm_body(mode: WizardMode, bindings: &FieldBindings) -> TokenStream2 {
    match mode {
        WizardMode::Skip => quote! { None },
        WizardMode::Popup => {
            let pc = bindings.popup_content.as_ref().unwrap();
            quote! {
                Some(WizardOffer::Popup(#pc.clone()))
            }
        }
        WizardMode::Pane => {
            let pt = bindings.pane_title.as_ref().unwrap();
            let pc = bindings.pane_content.as_ref().unwrap();
            quote! {
                Some(WizardOffer::Pane {
                    title: #pt.clone(),
                    lines: #pc.clone(),
                })
            }
        }
        WizardMode::Both => {
            let popup = bindings.popup_content.as_ref().unwrap();
            let pt = bindings.pane_title.as_ref().unwrap();
            let pc = bindings.pane_content.as_ref().unwrap();
            quote! {
                Some(WizardOffer::Both {
                    popup: #popup.clone(),
                    pane_title: #pt.clone(),
                    pane_lines: #pc.clone(),
                })
            }
        }
    }
}

fn derive_enum(name: &Ident, data: &syn::DataEnum) -> Result<TokenStream2> {
    let mut arms = Vec::new();

    for variant in &data.variants {
        let vname = &variant.ident;
        let mode = parse_wizard_mode(&variant.attrs)?.ok_or_else(|| {
            Error::new_spanned(
                variant,
                format!(
                    "variant `{vname}` is missing a #[wizard(...)] attribute; \
                     use #[wizard(skip)] if it should not offer wizard content"
                ),
            )
        })?;

        let bindings = collect_field_bindings(&variant.fields)?;
        validate_bindings(mode, &bindings, variant.ident.span())?;

        // Build the destructure pattern — we only need to bind annotated fields.
        let arm_body = gen_arm_body(mode, &bindings);

        let pattern = match &variant.fields {
            Fields::Named(_) => {
                // Collect field names we actually reference.
                let mut bound: Vec<&Ident> = Vec::new();
                if let Some(ref f) = bindings.popup_content {
                    bound.push(f);
                }
                if let Some(ref f) = bindings.pane_title {
                    bound.push(f);
                }
                if let Some(ref f) = bindings.pane_content {
                    bound.push(f);
                }
                if bound.is_empty() {
                    quote! { Self::#vname { .. } }
                } else {
                    quote! { Self::#vname { #(#bound,)* .. } }
                }
            }
            Fields::Unit => quote! { Self::#vname },
            Fields::Unnamed(_) => {
                return Err(Error::new_spanned(
                    variant,
                    "WizardItem only supports named fields (not tuple variants)",
                ));
            }
        };

        arms.push(quote! {
            #pattern => { #arm_body }
        });
    }

    Ok(quote! {
        impl WizardItem for #name {
            fn wizard(&self, _width: u16) -> Option<WizardOffer> {
                match self {
                    #(#arms)*
                }
            }
        }
    })
}

fn derive_struct(name: &Ident, input: &DeriveInput, fields: &Fields) -> Result<TokenStream2> {
    let mode = parse_wizard_mode(&input.attrs)?.ok_or_else(|| {
        Error::new_spanned(
            name,
            "struct-level #[wizard(...)] attribute required (e.g., #[wizard(popup)])",
        )
    })?;

    let bindings = collect_field_bindings(fields)?;
    validate_bindings(mode, &bindings, name.span())?;

    // For structs, field access is self.field rather than destructured binding.
    let body = match mode {
        WizardMode::Skip => quote! { None },
        WizardMode::Popup => {
            let pc = bindings.popup_content.as_ref().unwrap();
            quote! {
                Some(WizardOffer::Popup(self.#pc.clone()))
            }
        }
        WizardMode::Pane => {
            let pt = bindings.pane_title.as_ref().unwrap();
            let pc = bindings.pane_content.as_ref().unwrap();
            quote! {
                Some(WizardOffer::Pane {
                    title: self.#pt.clone(),
                    lines: self.#pc.clone(),
                })
            }
        }
        WizardMode::Both => {
            let popup = bindings.popup_content.as_ref().unwrap();
            let pt = bindings.pane_title.as_ref().unwrap();
            let pc = bindings.pane_content.as_ref().unwrap();
            quote! {
                Some(WizardOffer::Both {
                    popup: self.#popup.clone(),
                    pane_title: self.#pt.clone(),
                    pane_lines: self.#pc.clone(),
                })
            }
        }
    };

    Ok(quote! {
        impl WizardItem for #name {
            fn wizard(&self, _width: u16) -> Option<WizardOffer> {
                #body
            }
        }
    })
}
