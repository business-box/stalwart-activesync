use crate::page::{Page, Tag, UnknownTag};
use std::fmt::{self, Write as _};

/// A child of an [`Element`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    /// Inline string (`STR_I`).
    Text(String),
    /// Binary data (`OPAQUE`), e.g. MIME bodies or `ConversationId`.
    Opaque(Vec<u8>),
}

/// A WBXML element. EAS uses no attributes, so an element is a tag plus
/// children.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    pub tag: Tag,
    pub children: Vec<Node>,
}

impl Element {
    /// New element for a tag name that is known to exist, such as a string
    /// literal in handler code.
    ///
    /// # Panics
    /// If `name` is not defined on `page`. Use [`Element::try_new`] for names
    /// that come from input.
    pub fn new(page: Page, name: &str) -> Element {
        Element::try_new(page, name).unwrap_or_else(|e| panic!("{e}"))
    }

    pub fn try_new(page: Page, name: &str) -> Result<Element, UnknownTag> {
        Ok(Element::from_tag(Tag::named(page, name)?))
    }

    pub fn from_tag(tag: Tag) -> Element {
        Element {
            tag,
            children: Vec::new(),
        }
    }

    pub fn page(&self) -> Page {
        self.tag.page
    }

    /// Tag name; `"?"` for a token the code page tables don't know.
    pub fn name(&self) -> &'static str {
        self.tag.name().unwrap_or("?")
    }

    /// True if this element is `name` on `page`.
    pub fn is(&self, page: Page, name: &str) -> bool {
        self.tag.page == page && self.tag.name() == Some(name)
    }

    pub fn with_child(mut self, child: Element) -> Element {
        self.children.push(Node::Element(child));
        self
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Element {
        self.children.push(Node::Text(text.into()));
        self
    }

    pub fn with_opaque(mut self, data: impl Into<Vec<u8>>) -> Element {
        self.children.push(Node::Opaque(data.into()));
        self
    }

    /// Appends `<name>text</name>` on the same page as `self`.
    pub fn with_text_child(self, name: &str, text: impl Into<String>) -> Element {
        let page = self.tag.page;
        self.with_child(Element::new(page, name).with_text(text))
    }

    pub fn push(&mut self, child: Element) {
        self.children.push(Node::Element(child));
    }

    /// Child elements, skipping text and opaque nodes.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// First child element named `name` on `page`.
    pub fn child(&self, page: Page, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.is(page, name))
    }

    /// All child elements named `name` on `page`.
    pub fn children_named<'a>(
        &'a self,
        page: Page,
        name: &'a str,
    ) -> impl Iterator<Item = &'a Element> + 'a {
        self.elements().filter(move |e| e.is(page, name))
    }

    /// Concatenated text content, or `None` if the element has no text node.
    /// Opaque data that is valid UTF-8 counts as text, because some clients
    /// send strings as opaque.
    pub fn text(&self) -> Option<String> {
        let mut out: Option<String> = None;
        for n in &self.children {
            let s = match n {
                Node::Text(t) => t.as_str(),
                Node::Opaque(b) => match std::str::from_utf8(b) {
                    Ok(s) => s,
                    Err(_) => continue,
                },
                Node::Element(_) => continue,
            };
            out.get_or_insert_with(String::new).push_str(s);
        }
        out
    }

    /// Text of the first child `name` on `page`.
    pub fn child_text(&self, page: Page, name: &str) -> Option<String> {
        self.child(page, name).and_then(Element::text)
    }

    /// Opaque bytes, or text bytes if the element holds a string.
    pub fn bytes(&self) -> Option<Vec<u8>> {
        let mut out: Option<Vec<u8>> = None;
        for n in &self.children {
            let b: &[u8] = match n {
                Node::Text(t) => t.as_bytes(),
                Node::Opaque(b) => b,
                Node::Element(_) => continue,
            };
            out.get_or_insert_with(Vec::new).extend_from_slice(b);
        }
        out
    }

    /// Pretty XML rendering for logs and tests. Opaque data is shown as a
    /// length, never as content, so bodies don't leak into logs.
    pub fn to_xml(&self) -> String {
        let mut s = String::new();
        self.write_xml(&mut s, 0, None);
        s
    }

    fn write_xml(&self, out: &mut String, depth: usize, parent_page: Option<Page>) {
        let indent = "  ".repeat(depth);
        let _ = write!(out, "{indent}<{}", self.name());
        if parent_page != Some(self.tag.page) {
            let _ = write!(out, " xmlns=\"{}:\"", self.tag.page.namespace());
        }
        if self.children.is_empty() {
            out.push_str("/>\n");
            return;
        }
        let only_leaf = self.children.iter().all(|n| !matches!(n, Node::Element(_)));
        out.push('>');
        if only_leaf {
            for n in &self.children {
                write_leaf(out, n);
            }
        } else {
            out.push('\n');
            for n in &self.children {
                match n {
                    Node::Element(e) => e.write_xml(out, depth + 1, Some(self.tag.page)),
                    leaf => {
                        out.push_str(&"  ".repeat(depth + 1));
                        write_leaf(out, leaf);
                        out.push('\n');
                    }
                }
            }
            out.push_str(&indent);
        }
        let _ = writeln!(out, "</{}>", self.name());
    }
}

fn write_leaf(out: &mut String, n: &Node) {
    match n {
        Node::Text(t) => {
            for c in t.chars() {
                match c {
                    '<' => out.push_str("&lt;"),
                    '>' => out.push_str("&gt;"),
                    '&' => out.push_str("&amp;"),
                    c => out.push(c),
                }
            }
        }
        Node::Opaque(b) => {
            let _ = write!(out, "[opaque {} bytes]", b.len());
        }
        Node::Element(_) => {}
    }
}

impl fmt::Display for Element {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.to_xml())
    }
}
