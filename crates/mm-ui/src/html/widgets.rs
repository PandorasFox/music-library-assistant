//! Widget state → HTML Node tree renderers.
//!
//! The web analogues of mm-tui's `render_*` functions. `HtmlRenderable` impls
//! for data types with natural HTML representations; standalone functions for
//! widgets that need external context (closures, button contexts).

use crate::html::style::style_to_inline_css;
use crate::html::{div, footer, h2, h3, hr, li, nav, p, section, span, table, tbody, td, th, thead, tr, ul, Node, Element, HtmlRenderable, a, button, header, label};
use crate::lateral_view::LateralView;
use crate::modal_buttons::{ButtonRowState, ModalButtons};
use crate::rich_text::{RichBlock, RichSpan};
use crate::standard_list::StandardListState;
use crate::text_input::TextInputState;
use crate::html::style::color_to_css;

// ============================================================================
// HtmlRenderable impls
// ============================================================================

impl HtmlRenderable for RichSpan {
    fn to_html_node(&self) -> Node {
        let css = style_to_inline_css(&self.style);
        let el = span().text(&self.text);
        if css.is_empty() {
            el.into()
        } else {
            el.attr("style", css).into()
        }
    }
}

impl HtmlRenderable for RichBlock {
    fn to_html_node(&self) -> Node {
        match self {
            RichBlock::Heading(s) => h3().class("mm-heading").text(s).into(),

            RichBlock::Paragraph(spans) => {
                p().class("mm-para")
                    .children(spans.iter().map(|s| s.to_html_node()))
                    .into()
            }

            RichBlock::Separator => hr().class("mm-sep").into(),

            RichBlock::Blank => div().class("mm-blank").into(),

            RichBlock::Table {
                headers,
                rows,
                col_ratio: _,
            } => {
                let head = thead().child(
                    tr().children(
                        headers
                            .iter()
                            .map(|h| th().child(h.to_html_node()).into()),
                    ),
                );

                let body = tbody().children(rows.iter().map(|row| {
                    tr().children(
                        row.iter()
                            .map(|cell| {
                                td().children(cell.iter().map(|s| s.to_html_node()))
                                    .into()
                            }),
                    )
                    .into()
                }));

                table()
                    .class("mm-table")
                    .child(head)
                    .child(body)
                    .into()
            }
        }
    }
}

// ============================================================================
// Standalone rendering functions
// ============================================================================

/// Render a sequence of `RichBlock`s into a container div.
pub fn render_rich_blocks(blocks: &[RichBlock]) -> Node {
    div()
        .class("mm-rich-blocks")
        .children(blocks.iter().map(|b| b.to_html_node()))
        .into()
}

/// Render a `StandardList` to HTML.
///
/// Only renders the visible window (scroll..scroll+visible_height).
/// The `render_item` closure provides per-item HTML, receiving
/// `(index, is_cursor, is_selected)`.
pub fn render_list<F>(
    state: &StandardListState,
    item_count: usize,
    render_item: F,
    title: &str,
    focused: bool,
) -> Node
where
    F: Fn(usize, bool, bool) -> Node,
{
    let visible = if state.visible_height == 0 {
        item_count
    } else {
        state.visible_height.min(item_count.saturating_sub(state.scroll))
    };

    let items: Vec<Node> = (0..visible)
        .map(|vi| {
            let idx = state.scroll + vi;
            let is_cursor = idx == state.cursor;
            let is_selected = state.selected.contains(&idx);
            let item_node = render_item(idx, is_cursor, is_selected);

            li().class("mm-list__item")
                .class_if("mm-list__item--cursor", is_cursor)
                .class_if("mm-list__item--selected", is_selected)
                .attr("data-idx", idx.to_string())
                .child(item_node)
                .into()
        })
        .collect();

    section()
        .class("mm-list")
        .class_if("mm-focused", focused)
        .attr("tabindex", "0")
        .attr("onkeydown", "window.__mm_list_nav(this,event)")
        .child(h2().class("mm-list__title").text(title))
        .child(
            ul().class("mm-list__items")
                .attr("onclick", "window.__mm_list_click(this,event)")
                .children(items),
        )
        .into()
}

/// Render a button row.
pub fn render_buttons<B: ModalButtons>(
    state: &ButtonRowState<B>,
    ctx: &B::Context,
) -> Node {
    let buttons: Vec<Node> = B::all()
        .iter()
        .map(|btn| {
            let is_active = *btn == state.selected;
            let enabled = btn.enabled(ctx);
            let btn_label = btn.label(ctx);
            let color = btn.color(ctx);
            let color_css = color_to_css(color);

            button()
                .class("mm-btn")
                .class_if("mm-btn--active", is_active)
                .class_if("mm-btn--disabled", !enabled)
                .attr("style", format!("border-color:{color_css}"))
                .bool_attr_if("disabled", !enabled)
                .text(btn_label.as_ref())
                .into()
        })
        .collect();

    nav().class("mm-buttons").children(buttons).into()
}

/// Render a text input field.
pub fn render_text_input(state: &TextInputState, input_label: &str, focused: bool) -> Node {
    let (before, cursor_ch, after) = state.cursor_splits();

    let mut value_el = div().class("mm-input__value");

    if !before.is_empty() {
        value_el = value_el.child(span().text(before));
    }

    let cursor_display: String = if cursor_ch == '\0' {
        " ".into()
    } else {
        cursor_ch.to_string()
    };
    value_el = value_el.child(
        span()
            .class("mm-input__cursor")
            .class_if("mm-input__cursor--focused", focused)
            .text(cursor_display),
    );

    if !after.is_empty() {
        value_el = value_el.child(span().text(after));
    }

    div()
        .class("mm-input")
        .class_if("mm-focused", focused)
        .child(label().class("mm-input__label").text(input_label))
        .child(value_el)
        .into()
}

/// Render the lateral view tab bar.
///
/// Each tab has an `onclick` that calls `window.__mm_navigate(label)`.
pub fn render_titlebar(active: LateralView, transactions_open: bool) -> Node {
    let views = LateralView::all(transactions_open);

    let tabs: Vec<Node> = views
        .iter()
        .map(|view| {
            let label = view.label();
            a().class("mm-tab")
                .class_if("mm-tab--active", *view == active)
                .attr("href", "#")
                .attr(
                    "onclick",
                    format!("event.preventDefault();window.__mm_navigate('{label}')"),
                )
                .text(label)
                .into()
        })
        .collect();

    header()
        .class("mm-titlebar")
        .child(nav().class("mm-titlebar__nav").children(tabs))
        .into()
}

/// Render a status bar footer.
pub fn render_status_bar(line1: Option<&str>, line2: Option<&str>) -> Node {
    let mut bar = footer().class("mm-status");
    if let Some(l1) = line1 {
        bar = bar.child(div().class("mm-status__line").text(l1));
    }
    if let Some(l2) = line2 {
        bar = bar.child(div().class("mm-status__line").text(l2));
    }
    bar.into()
}

// ============================================================================
// Element extension for conditional bool attrs
// ============================================================================

impl Element {
    /// Add a boolean attribute conditionally.
    pub fn bool_attr_if(self, name: &'static str, condition: bool) -> Self {
        if condition {
            self.bool_attr(name)
        } else {
            self
        }
    }
}
