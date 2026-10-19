//! Plain text and HTML helpers for body conversion.

/// Longest prefix of `s` that fits in `max` bytes without splitting a
/// character.
pub fn truncate_utf8(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    s[..end].to_string()
}

/// Escapes text and keeps its line breaks, for devices that asked for HTML
/// when the message only has a text part.
pub fn text_to_html(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 64);
    out.push_str("<html><body>");
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\n' => out.push_str("<br>\n"),
            '\r' => {}
            c => out.push(c),
        }
    }
    out.push_str("</body></html>");
    out
}

fn decode_entity(name: &str) -> Option<char> {
    Some(match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "apos" => '\'',
        "nbsp" => ' ',
        _ => {
            let num = name.strip_prefix('#')?;
            let code = match num.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => num.parse().ok()?,
            };
            char::from_u32(code)?
        }
    })
}

/// Rough HTML to text for devices that only take plain text: drops scripts,
/// styles and tags, turns block ends into line breaks and decodes entities.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 2);
    let mut rest = html;
    while let Some(i) = rest.find(['<', '&']) {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        if rest.starts_with('&') {
            let end = rest[1..].find(';').map(|e| e + 1).filter(|e| *e <= 10);
            match end.and_then(|e| decode_entity(&rest[1..e]).map(|c| (c, e))) {
                Some((c, e)) => {
                    out.push(c);
                    rest = &rest[e + 1..];
                }
                None => {
                    out.push('&');
                    rest = &rest[1..];
                }
            }
            continue;
        }
        let Some(close) = rest.find('>') else {
            rest = "";
            break;
        };
        let tag = rest[1..close].trim().to_ascii_lowercase();
        let name: String = tag
            .trim_start_matches('/')
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        rest = &rest[close + 1..];
        if !tag.starts_with('/') && (name == "script" || name == "style") {
            let end_tag = format!("</{name}");
            match rest.to_ascii_lowercase().find(&end_tag) {
                Some(p) => {
                    rest = &rest[p..];
                    if let Some(c) = rest.find('>') {
                        rest = &rest[c + 1..];
                    }
                }
                None => rest = "",
            }
            continue;
        }
        let block = matches!(
            name.as_str(),
            "p" | "div"
                | "tr"
                | "li"
                | "h1"
                | "h2"
                | "h3"
                | "h4"
                | "h5"
                | "h6"
                | "blockquote"
                | "table"
        );
        if name == "br" || (block && tag.starts_with('/')) {
            out.push('\n');
        }
    }
    out.push_str(rest);
    // Collapse the blank-line runs that nested blocks leave behind.
    let mut text = String::with_capacity(out.len());
    let mut newlines = 0;
    for line in out.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            newlines += 1;
            if newlines > 1 {
                continue;
            }
        } else {
            newlines = 0;
        }
        text.push_str(line);
        text.push('\n');
    }
    text.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_respects_char_boundaries() {
        assert_eq!(truncate_utf8("grüße", 3), "gr");
        assert_eq!(truncate_utf8("abc", 10), "abc");
    }

    #[test]
    fn html_to_text_basics() {
        let html = "<html><head><style>p{color:red}</style></head><body><p>Hi &amp; bye</p><div>Line<br>two &#x263A;</div><script>x()</script></body></html>";
        assert_eq!(html_to_text(html), "Hi & bye\nLine\ntwo ☺");
        assert_eq!(html_to_text("a &unknown; b"), "a &unknown; b");
        assert_eq!(html_to_text("unterminated <b"), "unterminated");
    }

    #[test]
    fn text_to_html_escapes() {
        assert_eq!(
            text_to_html("a<b>\r\nc"),
            "<html><body>a&lt;b&gt;<br>\nc</body></html>"
        );
    }
}
