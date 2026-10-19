//! WBXML codec for Exchange ActiveSync.
//!
//! EAS bodies are WBXML 1.3 documents (MS-ASWBXML) with UTF-8 strings, no
//! string table and no attributes. Tags are identified by a code page
//! ([`Page`]) and a 6-bit token; this crate keeps that pair in each
//! [`Element`] and only resolves names when asked, so decoding is cheap and
//! never allocates tag names.
//!
//! ```
//! use eas_wbxml::{decode, encode, Element, Page};
//!
//! let req = Element::new(Page::FolderHierarchy, "FolderSync")
//!     .with_child(Element::new(Page::FolderHierarchy, "SyncKey").with_text("0"));
//! let bytes = encode(&req);
//! assert_eq!(bytes, [0x03, 0x01, 0x6A, 0x00, 0x00, 0x07, 0x56, 0x52, 0x03, b'0', 0x00, 0x01, 0x01]);
//! assert_eq!(decode(&bytes).unwrap(), req);
//! ```
//!
//! The decoder treats its input as hostile: it is iterative (no recursion),
//! bounds depth and node count ([`Limits`]) and never trusts a length prefix
//! beyond the bytes it actually has.

#[rustfmt::skip]
mod codepages;
mod decode;
mod encode;
mod page;
mod tree;

pub use decode::{decode, decode_with_limits, DecodeError, Limits};
pub use encode::encode;
pub use page::{Page, Tag, UnknownTag};
pub use tree::{Element, Node};

/// MIME type of WBXML request and response bodies.
pub const CONTENT_TYPE: &str = "application/vnd.ms-sync.wbxml";

/// Global WBXML tokens used by EAS (WBXML 1.3, section 7.1).
pub(crate) mod token {
    pub const SWITCH_PAGE: u8 = 0x00;
    pub const END: u8 = 0x01;
    pub const STR_I: u8 = 0x03;
    pub const OPAQUE: u8 = 0xC3;
    pub const WITH_CONTENT: u8 = 0x40;
    pub const WITH_ATTRIBUTES: u8 = 0x80;
    pub const TAG_MASK: u8 = 0x3F;
}

/// Document header EAS always uses: WBXML 1.3, unknown public ID, UTF-8, empty
/// string table.
pub(crate) const HEADER: [u8; 4] = [0x03, 0x01, 0x6A, 0x00];
