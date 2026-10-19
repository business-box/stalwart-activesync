use crate::page::{Page, Tag};
use crate::token;
use crate::tree::{Element, Node};

/// Bounds applied while decoding untrusted input. Total input size is the
/// HTTP layer's job.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    /// Maximum element nesting depth.
    pub max_depth: usize,
    /// Maximum number of nodes (elements, strings, opaque blocks).
    pub max_nodes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            max_depth: 64,
            max_nodes: 500_000,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DecodeError {
    #[error("unexpected end of input at byte {0}")]
    Eof(usize),
    #[error("unsupported WBXML version 0x{0:02X}")]
    Version(u8),
    #[error("unsupported charset {0} (only UTF-8 is used by EAS)")]
    Charset(u32),
    #[error("multi-byte integer overflow at byte {0}")]
    IntOverflow(usize),
    #[error("unknown code page {page} at byte {at}")]
    UnknownPage { page: u8, at: usize },
    #[error("token 0x{token:02X} at byte {at} is not used by EAS")]
    UnsupportedToken { token: u8, at: usize },
    #[error("tag with attributes at byte {0}; EAS uses no attributes")]
    Attributes(usize),
    #[error("{what} outside the root element at byte {at}")]
    OutsideRoot { what: &'static str, at: usize },
    #[error("unterminated string at byte {0}")]
    UnterminatedString(usize),
    #[error("opaque length {len} at byte {at} exceeds remaining input")]
    OpaqueTooLong { len: u32, at: usize },
    #[error("nesting deeper than {0}")]
    TooDeep(usize),
    #[error("more than {0} nodes")]
    TooManyNodes(usize),
    #[error("document has no root element")]
    Empty,
    #[error("{0} element(s) left open at end of input")]
    Unclosed(usize),
    #[error("trailing data after the root element at byte {0}")]
    Trailing(usize),
}

/// Decodes an EAS WBXML document with default [`Limits`].
pub fn decode(input: &[u8]) -> Result<Element, DecodeError> {
    decode_with_limits(input, &Limits::default())
}

/// Decodes an EAS WBXML document.
///
/// Tokens unknown to the code page tables are kept (their name renders as
/// `"?"`); handlers simply won't match them. Invalid UTF-8 in strings is
/// replaced rather than rejected, since some clients send Latin-1 subjects.
pub fn decode_with_limits(input: &[u8], limits: &Limits) -> Result<Element, DecodeError> {
    let mut r = Reader { buf: input, pos: 0 };

    let version = r.byte()?;
    if version != 0x03 && version != 0x02 && version != 0x01 {
        return Err(DecodeError::Version(version));
    }
    let _public_id = r.mb_u32()?;
    let charset = r.mb_u32()?;
    if charset != 106 {
        return Err(DecodeError::Charset(charset));
    }
    let strtbl = r.mb_u32()? as usize;
    r.skip(strtbl)?;

    let mut page = Page::AirSync;
    let mut stack: Vec<Element> = Vec::new();
    let mut root: Option<Element> = None;
    let mut nodes = 0usize;

    while r.pos < r.buf.len() {
        let at = r.pos;
        let b = r.byte()?;
        match b {
            token::SWITCH_PAGE => {
                let n = r.byte()?;
                page = Page::from_number(n).ok_or(DecodeError::UnknownPage { page: n, at })?;
            }
            token::END => {
                let el = stack
                    .pop()
                    .ok_or(DecodeError::OutsideRoot { what: "END", at })?;
                match stack.last_mut() {
                    Some(parent) => parent.children.push(Node::Element(el)),
                    None => {
                        root = Some(el);
                        break;
                    }
                }
            }
            token::STR_I => {
                let s = r.cstr()?;
                count(&mut nodes, limits)?;
                stack
                    .last_mut()
                    .ok_or(DecodeError::OutsideRoot { what: "string", at })?
                    .children
                    .push(Node::Text(s));
            }
            token::OPAQUE => {
                let len = r.mb_u32()?;
                if len as usize > r.remaining() {
                    return Err(DecodeError::OpaqueTooLong { len, at });
                }
                let data = r.take(len as usize)?.to_vec();
                count(&mut nodes, limits)?;
                stack
                    .last_mut()
                    .ok_or(DecodeError::OutsideRoot { what: "opaque", at })?
                    .children
                    .push(Node::Opaque(data));
            }
            0x02 | 0x04 | 0x40..=0x44 | 0x80..=0x84 | 0xC0..=0xC2 | 0xC4 => {
                return Err(DecodeError::UnsupportedToken { token: b, at });
            }
            _ => {
                if b & token::WITH_ATTRIBUTES != 0 {
                    return Err(DecodeError::Attributes(at));
                }
                count(&mut nodes, limits)?;
                let el = Element::from_tag(Tag {
                    page,
                    token: b & token::TAG_MASK,
                });
                if b & token::WITH_CONTENT != 0 {
                    if stack.len() >= limits.max_depth {
                        return Err(DecodeError::TooDeep(limits.max_depth));
                    }
                    stack.push(el);
                } else {
                    match stack.last_mut() {
                        Some(parent) => parent.children.push(Node::Element(el)),
                        None => {
                            root = Some(el);
                            break;
                        }
                    }
                }
            }
        }
    }

    match root {
        Some(root) if r.pos == r.buf.len() => Ok(root),
        Some(_) => Err(DecodeError::Trailing(r.pos)),
        None if stack.is_empty() => Err(DecodeError::Empty),
        None => Err(DecodeError::Unclosed(stack.len())),
    }
}

fn count(nodes: &mut usize, limits: &Limits) -> Result<(), DecodeError> {
    *nodes += 1;
    if *nodes > limits.max_nodes {
        Err(DecodeError::TooManyNodes(limits.max_nodes))
    } else {
        Ok(())
    }
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }

    fn byte(&mut self) -> Result<u8, DecodeError> {
        let b = *self.buf.get(self.pos).ok_or(DecodeError::Eof(self.pos))?;
        self.pos += 1;
        Ok(b)
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if n > self.remaining() {
            return Err(DecodeError::Eof(self.buf.len()));
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }

    fn skip(&mut self, n: usize) -> Result<(), DecodeError> {
        self.take(n).map(|_| ())
    }

    fn mb_u32(&mut self) -> Result<u32, DecodeError> {
        let start = self.pos;
        let mut v: u32 = 0;
        for _ in 0..5 {
            let b = self.byte()?;
            if v > (u32::MAX >> 7) {
                return Err(DecodeError::IntOverflow(start));
            }
            v = (v << 7) | u32::from(b & 0x7F);
            if b & 0x80 == 0 {
                return Ok(v);
            }
        }
        Err(DecodeError::IntOverflow(start))
    }

    fn cstr(&mut self) -> Result<String, DecodeError> {
        let start = self.pos;
        let rest = &self.buf[self.pos..];
        let end = rest
            .iter()
            .position(|b| *b == 0)
            .ok_or(DecodeError::UnterminatedString(start))?;
        self.pos += end + 1;
        Ok(String::from_utf8_lossy(&rest[..end]).into_owned())
    }
}
