//! HTML virtual DOM tree for web rendering.
//!
//! Provides `Node` and `Element` — a minimal virtual DOM that renders mm-ui
//! widget state to HTML strings. The web analogue of ratatui's Frame/Buffer.
//!
//! v1 uses full re-render via `innerHTML`. No diffing — the tree is rebuilt
//! on each state change.

pub mod style;
pub mod widgets;

// ============================================================================
// Core Types
// ============================================================================

/// A node in the virtual DOM tree.
pub enum Node {
    /// An HTML element with tag, attributes, and children.
    Element(Element),
    /// Text content (HTML-escaped on render).
    Text(String),
}

/// An HTML element.
pub struct Element {
    pub tag: &'static str,
    pub classes: Vec<&'static str>,
    pub attrs: Vec<(&'static str, String)>,
    pub children: Vec<Node>,
}

// ============================================================================
// HtmlRenderable Trait
// ============================================================================

/// Types that can render themselves as an HTML node tree.
///
/// For data types with a natural HTML representation (RichBlock, RichSpan,
/// status types, etc.). Widget renderers that need external context
/// (StandardList needs a render_item closure, ButtonRow needs button context)
/// use standalone functions in `html::widgets` instead.
///
/// Future: `#[derive(HtmlRenderable)]` in mm-derive following the same
/// attribute pattern as `WizardItem` derive.
pub trait HtmlRenderable {
    fn to_html_node(&self) -> Node;
}

// ============================================================================
// Builder API
// ============================================================================

/// Create an element with the given tag name.
pub fn el(tag: &'static str) -> Element {
    Element {
        tag,
        classes: Vec::new(),
        attrs: Vec::new(),
        children: Vec::new(),
    }
}

/// Create a text node.
pub fn text(t: impl Into<String>) -> Node {
    Node::Text(t.into())
}

// Convenience constructors for common HTML tags.
pub fn div() -> Element { el("div") }
pub fn span() -> Element { el("span") }
pub fn p() -> Element { el("p") }
pub fn pre() -> Element { el("pre") }
pub fn ul() -> Element { el("ul") }
pub fn ol() -> Element { el("ol") }
pub fn li() -> Element { el("li") }
pub fn button() -> Element { el("button") }
pub fn nav() -> Element { el("nav") }
pub fn header() -> Element { el("header") }
pub fn footer() -> Element { el("footer") }
pub fn section() -> Element { el("section") }
pub fn dialog() -> Element { el("dialog") }
pub fn h2() -> Element { el("h2") }
pub fn h3() -> Element { el("h3") }
pub fn hr() -> Element { el("hr") }
pub fn table() -> Element { el("table") }
pub fn thead() -> Element { el("thead") }
pub fn tbody() -> Element { el("tbody") }
pub fn tr() -> Element { el("tr") }
pub fn th() -> Element { el("th") }
pub fn td() -> Element { el("td") }
pub fn input() -> Element { el("input") }
pub fn label() -> Element { el("label") }
pub fn select() -> Element { el("select") }
pub fn option() -> Element { el("option") }
pub fn a() -> Element { el("a") }
pub fn details() -> Element { el("details") }
pub fn summary() -> Element { el("summary") }

impl Element {
    /// Add a CSS class.
    pub fn class(mut self, cls: &'static str) -> Self {
        self.classes.push(cls);
        self
    }

    /// Add a CSS class conditionally.
    pub fn class_if(mut self, cls: &'static str, condition: bool) -> Self {
        if condition {
            self.classes.push(cls);
        }
        self
    }

    /// Set the `id` attribute.
    pub fn id(self, id: &str) -> Self {
        self.attr("id", id)
    }

    /// Add an attribute.
    pub fn attr(mut self, name: &'static str, value: impl Into<String>) -> Self {
        self.attrs.push((name, value.into()));
        self
    }

    /// Add a boolean attribute (rendered as `name` with no value).
    pub fn bool_attr(mut self, name: &'static str) -> Self {
        self.attrs.push((name, String::new()));
        self
    }

    /// Add a child node.
    pub fn child(mut self, node: impl Into<Node>) -> Self {
        self.children.push(node.into());
        self
    }

    /// Add multiple child nodes.
    pub fn children(mut self, nodes: impl IntoIterator<Item = Node>) -> Self {
        self.children.extend(nodes);
        self
    }

    /// Add a text child.
    pub fn text(self, t: impl Into<String>) -> Self {
        self.child(Node::Text(t.into()))
    }
}

impl From<Element> for Node {
    fn from(el: Element) -> Node {
        Node::Element(el)
    }
}

// ============================================================================
// HTML Serialization
// ============================================================================

/// Void elements that self-close (no closing tag).
const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input",
    "link", "meta", "source", "track", "wbr",
];

impl Node {
    /// Render this node to an HTML string.
    pub fn to_html(&self) -> String {
        let mut buf = String::new();
        self.render_to(&mut buf);
        buf
    }

    /// Append this node's HTML to the buffer.
    pub fn render_to(&self, buf: &mut String) {
        match self {
            Node::Text(t) => escape_html(t, buf),
            Node::Element(el) => el.render_to(buf),
        }
    }
}

impl Element {
    fn render_to(&self, buf: &mut String) {
        buf.push('<');
        buf.push_str(self.tag);

        // Classes
        if !self.classes.is_empty() {
            buf.push_str(" class=\"");
            for (i, cls) in self.classes.iter().enumerate() {
                if i > 0 {
                    buf.push(' ');
                }
                buf.push_str(cls);
            }
            buf.push('"');
        }

        // Attributes
        for (name, value) in &self.attrs {
            buf.push(' ');
            buf.push_str(name);
            if !value.is_empty() {
                buf.push_str("=\"");
                escape_attr(value, buf);
                buf.push('"');
            }
        }

        let is_void = VOID_ELEMENTS.contains(&self.tag);

        if is_void {
            buf.push_str(">");
        } else {
            buf.push('>');
            for child in &self.children {
                child.render_to(buf);
            }
            buf.push_str("</");
            buf.push_str(self.tag);
            buf.push('>');
        }
    }
}

/// Escape text content for safe HTML embedding.
pub fn escape_html(s: &str, buf: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => buf.push_str("&amp;"),
            '<' => buf.push_str("&lt;"),
            '>' => buf.push_str("&gt;"),
            _ => buf.push(ch),
        }
    }
}

/// Escape attribute values for safe HTML embedding.
pub fn escape_attr(s: &str, buf: &mut String) {
    for ch in s.chars() {
        match ch {
            '&' => buf.push_str("&amp;"),
            '"' => buf.push_str("&quot;"),
            '<' => buf.push_str("&lt;"),
            '>' => buf.push_str("&gt;"),
            // Single quotes are safe inside double-quoted attributes and must
            // NOT be escaped — onclick handlers rely on them for JS string literals.
            _ => buf.push(ch),
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn simple_element() {
        let node = div().class("foo").text("hello");
        assert_eq!(node.into_node().to_html(), "<div class=\"foo\">hello</div>");
    }

    #[test]
    fn nested_elements() {
        let node = ul()
            .class("mm-list")
            .child(li().class("mm-list__item").text("one"))
            .child(li().class("mm-list__item").text("two"));
        assert_eq!(
            node.into_node().to_html(),
            "<ul class=\"mm-list\"><li class=\"mm-list__item\">one</li>\
             <li class=\"mm-list__item\">two</li></ul>"
        );
    }

    #[test]
    fn void_element() {
        let node = input().attr("type", "text").attr("value", "hi");
        assert_eq!(
            node.into_node().to_html(),
            "<input type=\"text\" value=\"hi\">"
        );
    }

    #[test]
    fn escaping() {
        let node = span().text("<script>alert('xss')</script>");
        assert_eq!(
            node.into_node().to_html(),
            "<span>&lt;script&gt;alert('xss')&lt;/script&gt;</span>"
        );
    }

    #[test]
    fn attr_escaping() {
        let node = div().attr("data-val", "a\"b<c");
        assert_eq!(
            node.into_node().to_html(),
            "<div data-val=\"a&quot;b&lt;c\"></div>"
        );
    }

    #[test]
    fn bool_attr() {
        let node = button().bool_attr("disabled").text("nope");
        assert_eq!(
            node.into_node().to_html(),
            "<button disabled>nope</button>"
        );
    }

    #[test]
    fn class_if() {
        let node = div().class("base").class_if("active", true).class_if("hidden", false);
        assert_eq!(
            node.into_node().to_html(),
            "<div class=\"base active\"></div>"
        );
    }

    #[test]
    fn text_node() {
        let node = text("hello & goodbye");
        assert_eq!(node.to_html(), "hello &amp; goodbye");
    }

    impl Element {
        fn into_node(self) -> Node {
            Node::Element(self)
        }
    }
}
