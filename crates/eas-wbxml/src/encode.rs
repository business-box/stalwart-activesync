use crate::token;
use crate::tree::{Element, Node};
use crate::HEADER;

/// Encodes `root` as a complete EAS WBXML document.
///
/// Text containing NUL characters cannot be sent as `STR_I`; such NULs are
/// dropped, matching what Exchange and Z-Push do.
pub fn encode(root: &Element) -> Vec<u8> {
    let mut out = Vec::with_capacity(256);
    out.extend_from_slice(&HEADER);
    let mut page: u8 = 0;

    // Iterative pre-order walk: (element, index of next child to emit).
    let mut stack: Vec<(&Element, usize)> = Vec::new();
    open(&mut out, &mut page, root);
    if root.children.is_empty() {
        return out;
    }
    stack.push((root, 0));

    while let Some((el, idx)) = stack.last_mut() {
        if *idx == el.children.len() {
            out.push(token::END);
            stack.pop();
            continue;
        }
        let node = &el.children[*idx];
        *idx += 1;
        match node {
            Node::Element(child) => {
                open(&mut out, &mut page, child);
                if !child.children.is_empty() {
                    stack.push((child, 0));
                }
            }
            Node::Text(t) => {
                out.push(token::STR_I);
                out.extend(t.bytes().filter(|b| *b != 0));
                out.push(0);
            }
            Node::Opaque(b) => {
                out.push(token::OPAQUE);
                write_mb_u32(&mut out, b.len() as u32);
                out.extend_from_slice(b);
            }
        }
    }
    out
}

fn open(out: &mut Vec<u8>, page: &mut u8, el: &Element) {
    let p = el.tag.page.number();
    if p != *page {
        out.push(token::SWITCH_PAGE);
        out.push(p);
        *page = p;
    }
    let mut t = el.tag.token & token::TAG_MASK;
    if !el.children.is_empty() {
        t |= token::WITH_CONTENT;
    }
    out.push(t);
}

/// WBXML multi-byte unsigned integer: 7 bits per byte, most significant
/// first, high bit set on every byte but the last.
pub(crate) fn write_mb_u32(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 5];
    let mut i = buf.len();
    loop {
        i -= 1;
        buf[i] = (v & 0x7F) as u8;
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    let last = buf.len() - 1;
    for b in &mut buf[i..last] {
        *b |= 0x80;
    }
    out.extend_from_slice(&buf[i..]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mb_u32() {
        let enc = |v| {
            let mut o = Vec::new();
            write_mb_u32(&mut o, v);
            o
        };
        assert_eq!(enc(0), [0x00]);
        assert_eq!(enc(0x7F), [0x7F]);
        assert_eq!(enc(0x80), [0x81, 0x00]);
        assert_eq!(enc(0xA0), [0x81, 0x20]);
        assert_eq!(enc(u32::MAX), [0x8F, 0xFF, 0xFF, 0xFF, 0x7F]);
    }
}
